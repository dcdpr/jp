//! `jp vite serve`: run the dev server, and decide whether it may write.
//!
//! Before starting `vitepress dev`, `serve` installs the site's dependencies
//! with the project's package manager ([`install`]), without modifying an
//! existing lockfile and without network access when `--offline` is set.
//! It then updates the settings snapshot and checks the site for version skew
//! ([`problems`]):
//!
//! - For each enabled workflow, `jp ticket`, `jp rfd`, and `jp wiki` report the
//!   site revision embedded in this binary, so the site loaders match the
//!   formats those plugins write.
//! - Every `jp-vite`-owned file other than the settings snapshot matches the
//!   embedded copy, so no loader is left over from another release.
//!
//! A failed check still starts the site.
//! The error is passed via `JP_VITE_PROBLEM`: the RFD, ticket, and wiki pages
//! display it in place of their content, and the dev server refuses write
//! requests.
//! Writes are accepted only under `JP_VITE_SERVE`, which names the workspace
//! root the middleware runs `jp` in.
//!
//! While the site runs, `serve` polls the host for configuration changes every
//! [`POLL`] and rewrites the snapshot when the relevant settings change, so a
//! changed directory or vocabulary reaches the site without a restart.
//! The snapshot is never edited by hand, so it is rewritten without the
//! dirty-file checks `sync` performs before overwriting a file.

use std::{
    fmt, fs,
    io::{self, BufRead, BufReader, Read, Write},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use camino::Utf8Path;
use jp_plugin::message::{HostToPlugin, PluginToHost, ReadConfigRequest};
use serde_json::Value;

use crate::{
    git, init,
    options::Options,
    print_to, read_message, send,
    settings::{Settings, below, snapshot, source_dir},
    shown, site,
    sync::{self, Change},
};

/// How often the configuration is read again.
pub(crate) const POLL: Duration = Duration::from_secs(2);

/// How long the loop waits for something to happen before checking on the dev
/// server and the clock.
const TICK: Duration = Duration::from_millis(200);

/// Where `cargo install` finds the plugins.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// CLI flags for `jp vite serve`.
pub(crate) struct Flags {
    /// Install the site's dependencies without network access.
    pub offline: bool,

    /// Trailing arguments forwarded to `vitepress dev`.
    pub vitepress: Vec<String>,
}

/// Run the dev server for the project at `root` until the host sends a shutdown
/// message or the server exits.
///
/// Spawns a background reader on `stdin` so host messages are received while
/// dependencies install and the server runs.
/// All messages to the host are written to `stdout` on the current thread.
pub(crate) fn serve<R, W>(
    stdin: R,
    stdout: &mut W,
    root: &Utf8Path,
    options: &Options,
    config: &Value,
    flags: &Flags,
) -> Result<(), String>
where
    R: BufRead + Send + 'static,
    W: Write,
{
    let site_dir = source_dir(root, options)?;
    if let Some(reason) = unready(root, &site_dir) {
        return Err(reason);
    }

    let (events, received) = mpsc::channel();
    listen(stdin, events.clone());

    let installer = install(&site_dir, flags.offline);
    if !run_install(stdout, root, &site_dir, &installer, &events, &received)? {
        return Ok(());
    }

    let remote = git::remote(root);
    let (mut settings, problem) = prepare(stdout, root, options, config, &remote)?;

    let mut child = start(
        root,
        &site_dir,
        &flags.vitepress,
        problem.as_deref(),
        &events,
    )?;

    let mut last_read = Instant::now();
    let mut reading = false;
    let mut complaint = None;

    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("`vitepress dev`: {error}"))?
        {
            // Drain any remaining output lines buffered before the process exited.
            while let Ok(Event::Output { text, channel }) = received.try_recv() {
                print_to(stdout, &text, channel)?;
            }
            return finished(status);
        }

        // The whole configuration rather than `plugins.command`: the host
        // leaves an empty section out, and a path it can't find is an error,
        // which a project with no plugin settings would get on every poll.
        if !reading && last_read.elapsed() >= POLL {
            send(
                stdout,
                &PluginToHost::ReadConfig(ReadConfigRequest {
                    id: Some("reload".to_owned()),
                    path: None,
                    reload: true,
                }),
            )?;
            reading = true;
            last_read = Instant::now();
        }

        let event = match received.recv_timeout(TICK) {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                Event::Host(Err("the host went away".to_owned()))
            }
        };

        match event {
            Event::Output { text, channel } => print_to(stdout, &text, channel)?,

            Event::Host(Ok(HostToPlugin::Config(response))) => {
                reading = false;
                let said = refresh(root, options, &response.data, &remote, &mut settings);
                report(stdout, &mut complaint, said)?;
            }

            // A configuration saved halfway fails to load. The host keeps the
            // last one that loaded, and so does the snapshot.
            Event::Host(Ok(HostToPlugin::Error(error))) => {
                reading = false;
                report(stdout, &mut complaint, Err(error.message))?;
            }

            Event::Host(Ok(HostToPlugin::Shutdown) | Err(_)) => {
                stop(&mut child);
                return Ok(());
            }

            Event::Host(Ok(_)) => {}
        }
    }
}

/// Bring the snapshot up to date and check the site, telling the user what was
/// found.
///
/// Returns the settings, and the problem the site has to show, if any.
fn prepare(
    stdout: &mut impl Write,
    root: &Utf8Path,
    options: &Options,
    config: &Value,
    remote: &git::Remote,
) -> Result<(Settings, Option<String>), String> {
    let settings = snapshot(root, options, config, remote)?;
    if let Some(note) = write_snapshot(root, &settings, None)? {
        print_to(stdout, &note, "chrome")?;
    }

    let changes = sync::plan(root, site::owned(&settings), &site::owned_dir(&settings))?;
    let revisions = Revisions {
        rfd: settings.rfd.as_ref().map(|_| ask_revision(root, "rfd")),
        ticket: settings
            .ticket
            .as_ref()
            .map(|_| ask_revision(root, "ticket")),
        wiki: settings.wiki.as_ref().map(|_| ask_revision(root, "wiki")),
    };

    let problem = problems(&changes, &site::settings_path(&settings), &revisions);
    if let Some(problem) = &problem {
        print_to(
            stdout,
            &format!(
                "{problem}\n\nThe site starts, but shows this in place of the RFD, ticket, and \
                 wiki pages, and refuses edits.\n"
            ),
            "chrome",
        )?;
    }

    Ok((settings, problem))
}

/// Write the settings a reloaded configuration gives, when they changed,
/// returning a note saying so.
fn refresh(
    root: &Utf8Path,
    options: &Options,
    config: &Value,
    remote: &git::Remote,
    settings: &mut Settings,
) -> Result<Option<String>, String> {
    let Some(fresh) = reloaded(root, options, config, remote, settings)? else {
        return Ok(None);
    };

    let note = write_snapshot(root, &fresh, Some(settings))?;
    *settings = fresh;

    Ok(note)
}

/// Tell the user what a reload came to.
///
/// A configuration that can't be used leaves the last snapshot in place until
/// it is fixed, and is complained about once rather than every [`POLL`].
fn report(
    stdout: &mut impl Write,
    complaint: &mut Option<String>,
    said: Result<Option<String>, String>,
) -> Result<(), String> {
    match said {
        Ok(note) => {
            *complaint = None;
            if let Some(note) = note {
                print_to(stdout, &note, "chrome")?;
            }
        }
        Err(error) if complaint.as_ref() != Some(&error) => {
            print_to(
                stdout,
                &format!("The site keeps its settings: {error}\n"),
                "chrome",
            )?;
            *complaint = Some(error);
        }
        Err(_) => {}
    }

    Ok(())
}

/// What happened while the site runs.
enum Event {
    /// A line the dev server printed, for the host's `channel`.
    Output { text: String, channel: &'static str },

    /// A message from the host, or why none can arrive.
    Host(Result<HostToPlugin, String>),
}

/// Spawn a thread that forwards each line from `output` to the host's `channel`
/// until EOF.
fn forward(
    output: impl Read + Send + 'static,
    channel: &'static str,
    events: Sender<Event>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let Ok(line) = line else { break };
            let text = format!("{line}\n");
            if events.send(Event::Output { text, channel }).is_err() {
                break;
            }
        }
    })
}

/// Spawn a thread that reads host messages from `stdin` and forwards them to
/// `events` until EOF or a read error.
fn listen(mut stdin: impl BufRead + Send + 'static, events: Sender<Event>) {
    thread::spawn(move || {
        loop {
            let message = read_message(&mut stdin);
            let over = message.is_err();
            if events.send(Event::Host(message)).is_err() || over {
                break;
            }
        }
    });
}

/// Start `vitepress dev` in `site_dir`.
///
/// Closes the child's stdin (since the plugin's stdin carries the host
/// protocol) and forwards its stdout and stderr to `events`.
fn start(
    root: &Utf8Path,
    site_dir: &Utf8Path,
    vitepress: &[String],
    problem: Option<&str>,
    events: &Sender<Event>,
) -> Result<Child, String> {
    let runner = runner(site_dir);
    let mut command = Command::new(runner[0]);
    command
        .args(&runner[1..])
        .arg("dev")
        .args(vitepress)
        .current_dir(site_dir)
        .env("JP_VITE_SERVE", root.as_str())
        .env_remove("JP_VITE_PROBLEM")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(problem) = problem {
        command.env("JP_VITE_PROBLEM", problem);
    }

    let mut child = command
        .spawn()
        .map_err(|error| format!("`{} dev` couldn't be started: {error}", runner.join(" ")))?;
    if let Some(output) = child.stdout.take() {
        forward(output, "content", events.clone());
    }
    if let Some(output) = child.stderr.take() {
        forward(output, "chrome", events.clone());
    }

    Ok(child)
}

/// Kill `child` and reap its process.
///
/// Any descendants spawned by `child` are terminated by the host when the
/// plugin exits and the host tears down the plugin's process tree.
fn stop(child: &mut Child) {
    drop(child.kill());
    drop(child.wait());
}

/// How the plugin ends once the dev server has.
fn finished(status: ExitStatus) -> Result<(), String> {
    if status.success() {
        Ok(())
    } else {
        Err(format!("`vitepress dev` stopped: {status}."))
    }
}

/// Check whether `site_dir` has a `package.json` ready to serve, returning an
/// error message when it does not.
///
/// If none of the [`init::TEMPLATES`] files exist, reports that no site is
/// initialized; otherwise lists the missing template files.
pub(crate) fn unready(root: &Utf8Path, site_dir: &Utf8Path) -> Option<String> {
    if site_dir.join("package.json").is_file() {
        return None;
    }

    let missing = init::missing(site_dir);
    if missing.len() == init::TEMPLATES.len() {
        return Some(format!(
            "There is no VitePress site in {}: run `jp vite init` to set one up.",
            shown(root, site_dir)
        ));
    }

    let paths: Vec<_> = missing
        .iter()
        .map(|template| {
            let path = site_dir.join(template.path);
            below(root, &path).unwrap_or_else(|| path.to_string())
        })
        .collect();
    let verb = if paths.len() == 1 { "is" } else { "are" };

    Some(format!(
        "{} {verb} missing, re-run `jp vite init`.",
        paths.join(", ")
    ))
}

/// JavaScript package manager used to install dependencies and run VitePress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Manager {
    Npm,
    Pnpm,
    Bun,

    /// Yarn 2 and later.
    Yarn,

    /// Yarn 1.
    YarnClassic,
}

impl Manager {
    fn program(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Bun => "bun",
            Self::Yarn | Self::YarnClassic => "yarn",
        }
    }

    /// Lockfile filenames recognized by this package manager, in priority
    /// order.
    fn lockfiles(self) -> &'static [&'static str] {
        match self {
            Self::Npm => &["npm-shrinkwrap.json", "package-lock.json"],
            Self::Pnpm => &["pnpm-lock.yaml"],
            Self::Bun => &["bun.lock", "bun.lockb"],
            Self::Yarn | Self::YarnClassic => &["yarn.lock"],
        }
    }
}

/// Detect the package manager for `site_dir` from `packageManager` in
/// `package.json`, falling back to lockfile detection and then npm.
pub(crate) fn manager(site_dir: &Utf8Path) -> Manager {
    let named = fs::read_to_string(site_dir.join("package.json"))
        .ok()
        .and_then(|source| serde_json::from_str::<Value>(&source).ok())
        .and_then(|package| Some(package.get("packageManager")?.as_str()?.to_owned()));

    if let Some(named) = named {
        let (name, version) = named.split_once('@').unwrap_or((&named, ""));
        return match name {
            "yarn" if version.starts_with("1.") => Manager::YarnClassic,
            "yarn" if version.is_empty() => yarn(site_dir),
            "yarn" => Manager::Yarn,
            "pnpm" => Manager::Pnpm,
            "bun" => Manager::Bun,
            _ => Manager::Npm,
        };
    }

    if site_dir.join("yarn.lock").is_file() {
        yarn(site_dir)
    } else if site_dir.join("pnpm-lock.yaml").is_file() {
        Manager::Pnpm
    } else if Manager::Bun
        .lockfiles()
        .iter()
        .any(|lockfile| site_dir.join(lockfile).is_file())
    {
        Manager::Bun
    } else {
        Manager::Npm
    }
}

/// Distinguish Yarn 2+ from Yarn 1 by inspecting `yarn.lock` in `site_dir`.
///
/// Lockfiles generated by Yarn 2+ contain a `__metadata:` key; Yarn 1 lockfiles
/// do not.
/// Defaults to [`Manager::Yarn`] (Yarn 2+) when `yarn.lock` is absent.
fn yarn(site_dir: &Utf8Path) -> Manager {
    match fs::read_to_string(site_dir.join("yarn.lock")) {
        Ok(lockfile) if !lockfile.contains("__metadata:") => Manager::YarnClassic,
        _ => Manager::Yarn,
    }
}

/// Return the command and arguments used to invoke `vitepress` via the site's
/// [`manager`].
pub(crate) fn runner(site_dir: &Utf8Path) -> Vec<&'static str> {
    match manager(site_dir) {
        Manager::Yarn | Manager::YarnClassic => vec!["yarn", "vitepress"],
        Manager::Pnpm => vec!["pnpm", "exec", "vitepress"],
        Manager::Bun => vec!["bunx", "vitepress"],
        Manager::Npm => vec!["npx", "vitepress"],
    }
}

/// Command and environment for installing the site's dependencies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Install {
    pub manager: Manager,
    pub args: Vec<&'static str>,
    pub env: Vec<(&'static str, &'static str)>,

    /// Existing lockfile enforced during installation, if any.
    pub lockfile: Option<&'static str>,
    pub offline: bool,
}

impl fmt::Display for Install {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (name, value) in &self.env {
            write!(f, "{name}={value} ")?;
        }
        write!(f, "{} {}", self.manager.program(), self.args.join(" "))
    }
}

/// Build the dependency installation command for `site_dir` using its
/// [`manager`].
///
/// When a lockfile exists, passes the manager's immutable or frozen flag so the
/// install fails instead of modifying the lockfile.
/// When no lockfile exists, the install generates one.
/// When `offline` is `true`, disables network access so the install fails if
/// any dependency is missing from the local cache.
pub(crate) fn install(site_dir: &Utf8Path, offline: bool) -> Install {
    let manager = manager(site_dir);
    let lockfile = manager
        .lockfiles()
        .iter()
        .copied()
        .find(|lockfile| site_dir.join(lockfile).is_file());

    // pnpm and Yarn 2+ default to frozen/immutable installs in CI environments,
    // which fails when no lockfile exists yet. Pass the unlocked flag explicitly.
    let mut args = match (manager, lockfile.is_some()) {
        (Manager::Npm, true) => vec!["ci"],
        (Manager::Yarn, true) => vec!["install", "--immutable"],
        (Manager::Yarn, false) => vec!["install", "--no-immutable"],
        (Manager::Pnpm, false) => vec!["install", "--no-frozen-lockfile"],
        (Manager::Pnpm | Manager::Bun | Manager::YarnClassic, true) => {
            vec!["install", "--frozen-lockfile"]
        }
        (Manager::Npm | Manager::Bun | Manager::YarnClassic, false) => vec!["install"],
    };

    let mut env = vec![];
    if offline {
        // Corepack shims Yarn and pnpm and downloads the pinned `packageManager`
        // version when it is not cached locally.
        if matches!(
            manager,
            Manager::Yarn | Manager::YarnClassic | Manager::Pnpm
        ) {
            env.push(("COREPACK_ENABLE_NETWORK", "0"));
        }
        // Yarn 2+ has no `--offline` flag; disable networking via its
        // environment variable instead.
        if manager == Manager::Yarn {
            env.push(("YARN_ENABLE_NETWORK", "0"));
        } else {
            args.push("--offline");
        }
    }

    Install {
        manager,
        args,
        env,
        lockfile,
        offline,
    }
}

/// Run `install` in `site_dir`, forwarding its output to the host.
///
/// Returns `Ok(false)` if the host shuts down before the install completes.
fn run_install(
    stdout: &mut impl Write,
    root: &Utf8Path,
    site_dir: &Utf8Path,
    install: &Install,
    events: &Sender<Event>,
    received: &Receiver<Event>,
) -> Result<bool, String> {
    print_to(
        stdout,
        &format!("Installing the site's dependencies: `{install}`\n"),
        "chrome",
    )?;

    let child = Command::new(install.manager.program())
        .args(&install.args)
        .envs(install.env.iter().copied())
        .current_dir(site_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| unstarted(install, &error))?;

    let Some(status) = follow(stdout, child, events, received)? else {
        return Ok(false);
    };
    if !status.success() {
        return Err(failed(install, &shown(root, site_dir), status));
    }

    Ok(true)
}

/// Forward `child`'s stdout and stderr to the host's `chrome` channel until the
/// process exits, and return its [`ExitStatus`].
///
/// If the host shuts down first, kills `child` and returns `Ok(None)`.
fn follow(
    stdout: &mut impl Write,
    mut child: Child,
    events: &Sender<Event>,
    received: &Receiver<Event>,
) -> Result<Option<ExitStatus>, String> {
    let readers: Vec<_> = [
        child
            .stdout
            .take()
            .map(|output| forward(output, "chrome", events.clone())),
        child
            .stderr
            .take()
            .map(|output| forward(output, "chrome", events.clone())),
    ]
    .into_iter()
    .flatten()
    .collect();

    let Some(status) = wait(stdout, &mut child, received)? else {
        return Ok(None);
    };

    // Join the reader threads before draining `received` so any final error
    // output buffered in the pipes is forwarded before reporting the exit status.
    for reader in readers {
        drop(reader.join());
    }
    while let Ok(event) = received.try_recv() {
        match event {
            Event::Output { text, channel } => print_to(stdout, &text, channel)?,
            Event::Host(Ok(HostToPlugin::Shutdown) | Err(_)) => return Ok(None),
            Event::Host(Ok(_)) => {}
        }
    }

    Ok(Some(status))
}

/// Poll `child` until it exits while forwarding buffered output to the host.
///
/// If the host shuts down first, kills `child` and returns `Ok(None)`.
fn wait(
    stdout: &mut impl Write,
    child: &mut Child,
    received: &Receiver<Event>,
) -> Result<Option<ExitStatus>, String> {
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("waiting for the install: {error}"))?
        {
            return Ok(Some(status));
        }

        match received.recv_timeout(TICK) {
            Ok(Event::Output { text, channel }) => print_to(stdout, &text, channel)?,
            Ok(Event::Host(Ok(HostToPlugin::Shutdown) | Err(_)))
            | Err(RecvTimeoutError::Disconnected) => {
                stop(child);
                return Ok(None);
            }
            Ok(Event::Host(Ok(_))) | Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

/// Format an error message when spawning `install` fails with `error`.
///
/// When the package manager executable is not found on `PATH`, includes
/// instructions for installing or enabling it.
pub(crate) fn unstarted(install: &Install, error: &io::Error) -> String {
    if error.kind() != io::ErrorKind::NotFound {
        return format!("`{install}` couldn't be started: {error}");
    }

    let how = match install.manager {
        Manager::Npm => "Install Node.js, which comes with it",
        Manager::Bun => "Install Bun",
        Manager::Pnpm | Manager::Yarn | Manager::YarnClassic => {
            "Install it, or run `corepack enable` if your Node.js comes with Corepack"
        }
    };

    format!(
        "`{}` isn't on your PATH, and the site's dependencies are installed with it. {how}, then \
         run `jp vite serve` again.",
        install.manager.program()
    )
}

/// Format an error message when `install` exits with a non-zero `status` in
/// `site_dir`, appending remediation hints for lockfile mismatches and
/// `--offline` cache misses.
pub(crate) fn failed(install: &Install, site_dir: &Utf8Path, status: impl fmt::Display) -> String {
    let mut text = format!("`{install}` failed in {site_dir} ({status}).");

    if let Some(lockfile) = install.lockfile {
        text.push_str(&format!(
            " If {lockfile} doesn't match package.json, run `{} install` in {site_dir} to update \
             it.",
            install.manager.program()
        ));
    }
    if install.offline {
        text.push_str(
            " With `--offline`, a dependency missing from the package manager's cache fails the \
             install: run without it to download what's missing.",
        );
    }

    text
}

/// Write the snapshot when it differs from `previous`, or from the file when
/// there is no previous one, returning a note saying so.
fn write_snapshot(
    root: &Utf8Path,
    settings: &Settings,
    previous: Option<&Settings>,
) -> Result<Option<String>, String> {
    let path = root.join(site::settings_path(settings));
    let rendered = settings.render();

    let unchanged = match previous {
        Some(previous) => previous == settings,
        None => {
            fs::read(&path).is_ok_and(|current| overwrite::same_text(&current, rendered.as_bytes()))
        }
    };
    if unchanged {
        return Ok(None);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{parent}: {error}"))?;
    }
    fs::write(&path, rendered).map_err(|error| format!("{path}: {error}"))?;

    Ok(Some(format!(
        "Wrote the site's settings to {}.\n",
        site::settings_path(settings)
    )))
}

/// The settings a reloaded configuration gives, when they differ from
/// `current`.
///
/// The source directory is the one the dev server runs in, whatever the
/// configuration says now; a new one takes a restart.
fn reloaded(
    root: &Utf8Path,
    options: &Options,
    config: &Value,
    remote: &git::Remote,
    current: &Settings,
) -> Result<Option<Settings>, String> {
    let fresh_options = Options {
        dir: options.dir.clone(),
        ..Options::from_config(config)?
    };
    let fresh = snapshot(root, &fresh_options, config, remote)?;

    Ok((fresh != *current).then_some(fresh))
}

/// What `jp rfd --version`, `jp ticket --version`, and `jp wiki --version`
/// answered, for the workflows the project has.
pub(crate) struct Revisions {
    pub rfd: Option<Result<String, String>>,
    pub ticket: Option<Result<String, String>>,
    pub wiki: Option<Result<String, String>>,
}

/// Ask `jp <plugin> --version` what it prints.
///
/// Run where the host runs, with no terminal, so a plugin that needs approval
/// is refused rather than asked about.
fn ask_revision(root: &Utf8Path, plugin: &str) -> Result<String, String> {
    let output = Command::new("jp")
        .args(["--no-interactive", plugin, "--version"])
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("`jp` couldn't be run: {error}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let said = stderr
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("it failed without saying why");
        return Err(said.to_owned());
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The site revision in what `jp <plugin> --version` printed, which ends in
/// `(site <revision>)`.
pub(crate) fn reported_revision(version: &str) -> Option<&str> {
    let (_, rest) = version.split_once("(site ")?;
    let (revision, _) = rest.split_once(')')?;

    Some(revision.trim()).filter(|revision| !revision.is_empty())
}

/// Why the site can't be trusted to read its documents, if it can't.
///
/// `changes` are what syncing would change, `snapshot` the snapshot's path
/// among them, and `revisions` what the plugins reported.
pub(crate) fn problems(
    changes: &[Change],
    snapshot: &Utf8Path,
    revisions: &Revisions,
) -> Option<String> {
    let mut problems = vec![];

    for (plugin, kind, reported) in [
        ("rfd", "RFDs", &revisions.rfd),
        ("ticket", "tickets", &revisions.ticket),
        ("wiki", "wiki pages", &revisions.wiki),
    ] {
        if let Some(reported) = reported
            && let Some(problem) = revision_problem(plugin, kind, reported)
        {
            problems.push(problem);
        }
    }

    let stale: Vec<&Utf8Path> = changes
        .iter()
        .map(Change::path)
        .filter(|path| *path != snapshot)
        .collect();
    if !stale.is_empty() {
        let mut problem = String::from(
            "These files aren't the ones this `jp vite` writes, so the site may run with files \
             from another release:\n",
        );
        for path in stale {
            problem.push_str(&format!("  {path}\n"));
        }
        problem.push_str("Run `jp vite sync` to write them.");
        problems.push(problem);
    }

    (!problems.is_empty()).then(|| problems.join("\n\n"))
}

/// Why the site may misread what `jp <plugin>` writes, given what it reported.
fn revision_problem(plugin: &str, kind: &str, reported: &Result<String, String>) -> Option<String> {
    let install =
        format!("cargo install --git {REPOSITORY} --rev <rev> jp-ticket jp-rfd jp-wiki jp-vite");

    let output = match reported {
        Ok(output) => output,
        Err(error) => {
            return Some(format!(
                "`jp {plugin}` didn't answer ({error}). The site shows {kind}, and reads them the \
                 way `jp {plugin}` writes them. Install it from the JP revision `jp-vite` came \
                 from, in one command with the others:\n  {install}"
            ));
        }
    };

    match reported_revision(output) {
        Some(revision) if revision == site::REVISION => None,
        Some(revision) => Some(format!(
            "`jp {plugin}` was built with site files {revision}, and `jp vite` with {}, so the \
             site may misread the {kind} `jp {plugin}` writes. Install them all from one JP \
             revision:\n  {install}",
            site::REVISION
        )),
        None => Some(format!(
            "`jp {plugin}` reports no site revision, so it predates `jp vite`, and the site may \
             misread the {kind} it writes. Install them all from one JP revision:\n  {install}"
        )),
    }
}

#[cfg(test)]
#[path = "serve_tests.rs"]
mod tests;
