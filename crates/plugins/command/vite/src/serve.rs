//! `jp vite serve`: run the dev server, and decide whether it may write.
//!
//! Before starting `vitepress dev`, serve brings the settings snapshot up to
//! date and checks the site against what it reads ([`problems`]):
//!
//! - `jp ticket` and `jp rfd`, for each workflow the project has, report the
//!   revision of the site files this binary carries, so the loaders read the
//!   formats those plugins write.
//! - Every owned file but the snapshot is the one this binary writes, so no
//!   loader is left over from another release.
//!
//! A failed check still starts the site.
//! The problem reaches it in `JP_VITE_PROBLEM`: the RFD and ticket pages show
//! it in place of their content, and the dev server refuses writes.
//! Writes are accepted only under `JP_VITE_SERVE`, which names the workspace
//! root the middleware runs `jp` in.
//!
//! While the site runs, serve asks the host for the configuration again every
//! [`POLL`] and rewrites the snapshot when the settings in it change, so a
//! changed directory or vocabulary reaches the site without a restart.
//! The snapshot holds nothing written by hand, so it is rewritten without the
//! checks `sync` makes before overwriting a file.

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, RecvTimeoutError, Sender},
    thread,
    time::{Duration, Instant},
};

use camino::Utf8Path;
use jp_plugin::message::{HostToPlugin, PluginToHost, ReadConfigRequest};
use serde_json::Value;

use crate::{
    git,
    options::Options,
    print_to, read_message, send,
    settings::{Settings, snapshot, source_dir},
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

/// Run the dev server for the project at `root` until the host asks it to stop,
/// or it stops on its own.
///
/// `stdin` goes to a thread of its own, so the host is heard while the server
/// runs; everything sent to the host goes through `stdout` from here.
/// `vitepress` are the arguments `vitepress dev` gets.
pub(crate) fn serve<R, W>(
    stdin: R,
    stdout: &mut W,
    root: &Utf8Path,
    options: &Options,
    config: &Value,
    vitepress: &[String],
) -> Result<(), String>
where
    R: BufRead + Send + 'static,
    W: Write,
{
    let site_dir = source_dir(root, options)?;
    if !site_dir.join("package.json").is_file() {
        return Err(format!(
            "There is no VitePress site in {}: run `jp vite init` to set one up.",
            shown(root, &site_dir)
        ));
    }

    let origin = git::origin(root);
    let (mut settings, problem) = prepare(stdout, root, options, config, origin.as_deref())?;

    let mut child = start(root, &site_dir, vitepress, problem.as_deref())?;

    let (events, received) = mpsc::channel();
    if let Some(output) = child.stdout.take() {
        forward(output, "content", events.clone());
    }
    if let Some(output) = child.stderr.take() {
        forward(output, "chrome", events.clone());
    }
    listen(stdin, events);

    let mut last_read = Instant::now();
    let mut reading = false;
    let mut complaint = None;

    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("`vitepress dev`: {error}"))?
        {
            // What it printed on the way out is still in the channel.
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
                let said = refresh(
                    root,
                    options,
                    &response.data,
                    origin.as_deref(),
                    &mut settings,
                );
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
    origin: Option<&str>,
) -> Result<(Settings, Option<String>), String> {
    let settings = snapshot(root, options, config, origin)?;
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
    };

    let problem = problems(&changes, &site::settings_path(&settings), &revisions);
    if let Some(problem) = &problem {
        print_to(
            stdout,
            &format!(
                "{problem}\n\nThe site starts, but shows this in place of the RFD and ticket \
                 pages, and refuses edits.\n"
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
    origin: Option<&str>,
    settings: &mut Settings,
) -> Result<Option<String>, String> {
    let Some(fresh) = reloaded(root, options, config, origin, settings)? else {
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

/// Forward each line of `output` to the host's `channel`.
fn forward(output: impl Read + Send + 'static, channel: &'static str, events: Sender<Event>) {
    thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let Ok(line) = line else { break };
            let text = format!("{line}\n");
            if events.send(Event::Output { text, channel }).is_err() {
                break;
            }
        }
    });
}

/// Pass on what the host says, until it stops saying anything.
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

/// Start `vitepress dev` in the source directory.
///
/// Its stdin is closed, since the plugin's own carries the protocol, and its
/// output is forwarded to the host, which owns the terminal.
fn start(
    root: &Utf8Path,
    site_dir: &Utf8Path,
    vitepress: &[String],
    problem: Option<&str>,
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

    command
        .spawn()
        .map_err(|error| format!("`{} dev` couldn't be started: {error}", runner.join(" ")))
}

/// Stop the dev server.
///
/// What it started goes with the plugin: the host ends the plugin's whole
/// process tree once it exits.
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

/// The command that runs the project's `vitepress`, through the package manager
/// the project uses: the one `packageManager` in `package.json` names, or the
/// one whose lockfile is there, or `npx`.
pub(crate) fn runner(site_dir: &Utf8Path) -> Vec<&'static str> {
    let named = fs::read_to_string(site_dir.join("package.json"))
        .ok()
        .and_then(|source| serde_json::from_str::<Value>(&source).ok())
        .and_then(|package| {
            let manager = package.get("packageManager")?.as_str()?;
            Some(manager.split('@').next().unwrap_or_default().to_owned())
        });

    let manager = named.or_else(|| {
        [
            ("yarn.lock", "yarn"),
            ("pnpm-lock.yaml", "pnpm"),
            ("bun.lock", "bun"),
            ("bun.lockb", "bun"),
        ]
        .into_iter()
        .find(|(lockfile, _)| site_dir.join(lockfile).is_file())
        .map(|(_, manager)| manager.to_owned())
    });

    match manager.as_deref() {
        Some("yarn") => vec!["yarn", "vitepress"],
        Some("pnpm") => vec!["pnpm", "exec", "vitepress"],
        Some("bun") => vec!["bunx", "vitepress"],
        _ => vec!["npx", "vitepress"],
    }
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
        None => fs::read(&path).is_ok_and(|current| sync::same_text(&current, rendered.as_bytes())),
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
    origin: Option<&str>,
    current: &Settings,
) -> Result<Option<Settings>, String> {
    let fresh_options = Options {
        dir: options.dir.clone(),
        ..Options::from_config(config)?
    };
    let fresh = snapshot(root, &fresh_options, config, origin)?;

    Ok((fresh != *current).then_some(fresh))
}

/// What `jp rfd --version` and `jp ticket --version` answered, for the
/// workflows the project has.
pub(crate) struct Revisions {
    pub rfd: Option<Result<String, String>>,
    pub ticket: Option<Result<String, String>>,
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
    let install = format!("cargo install --git {REPOSITORY} --rev <rev> jp-ticket jp-rfd jp-vite");

    let output = match reported {
        Ok(output) => output,
        Err(error) => {
            return Some(format!(
                "`jp {plugin}` didn't answer ({error}). The site shows {kind}, and reads them the \
                 way `jp {plugin}` writes them. Install it from the JP revision `jp-vite` came \
                 from, in one command with the other two:\n  {install}"
            ));
        }
    };

    match reported_revision(output) {
        Some(revision) if revision == site::REVISION => None,
        Some(revision) => Some(format!(
            "`jp {plugin}` was built with site files {revision}, and `jp vite` with {}, so the \
             site may misread the {kind} `jp {plugin}` writes. Install the three from one JP \
             revision:\n  {install}",
            site::REVISION
        )),
        None => Some(format!(
            "`jp {plugin}` reports no site revision, so it predates `jp vite`, and the site may \
             misread the {kind} it writes. Install the three from one JP revision:\n  {install}"
        )),
    }
}

#[cfg(test)]
#[path = "serve_tests.rs"]
mod tests;
