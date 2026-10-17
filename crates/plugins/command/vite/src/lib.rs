//! `jp-vite`: set up, sync, and serve the site that renders a workspace's RFDs
//! and tickets.
//!
//! A command plugin providing three commands: `jp vite init`, `jp vite sync`,
//! and `jp vite serve`.
//! The site's own files are built into the binary ([`site`]): the loaders for
//! both document kinds, the components, the styles, the dev-server middleware,
//! and a configuration fragment, which go to the project's
//! `.vitepress/jp-vite/` with a snapshot of the settings they read
//! ([`settings`]); and a starting index and board page for the RFD and ticket
//! directories.
//! `init` writes the files that stay the project's own ([`init`]); `sync`
//! rewrites the owned ones and creates the pages a project doesn't have
//! ([`sync`]), only where git can restore what it overwrites ([`git`]); and
//! `serve` runs the dev server ([`serve`]).
//!
//! Settings come from `plugins.command.vite.options`, and from `jp rfd`'s and
//! `jp ticket`'s options ([`options`]).
//!
//! See: `docs/rfd/116-shareable-rfd-ticket-and-site-tooling.md`, "Site"

mod git;
mod init;
mod options;
mod serve;
mod settings;
mod site;
mod sync;

use std::{
    io::{BufRead, Write},
    iter,
};

use camino::{Utf8Path, Utf8PathBuf};
use clap::{CommandFactory, Parser, Subcommand};
use jp_plugin::message::{
    ComposeMode, ComposeOption, ComposeRequest, DescribeResponse, ExitMessage, HostToPlugin,
    InitMessage, PluginToHost, PrintMessage,
};
use serde_json::Value;

use crate::{
    git::Unrecorded,
    options::Options,
    settings::{snapshot, source_dir},
    sync::Change,
};

/// What `jp vite --version` prints after the command's name: the plugin's
/// version, and the revision of the site files it carries.
const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (site ",
    env!("JP_SITE_REVISION"),
    ")"
);

#[derive(Debug, Parser)]
#[command(
    name = "jp vite",
    version = VERSION,
    about = "Set up, sync, and serve the site that renders RFDs and tickets."
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Set up a VitePress site in the source directory.
    ///
    /// Writes `package.json`, `.vitepress/config.mts`, the theme entry, and a
    /// homepage, which are yours from then on.
    /// A file that exists is left as it is.
    Init {
        /// The site's title.
        /// Asked for when missing.
        #[arg(long)]
        title: Option<String>,

        /// One line describing the site.
        /// Asked for when missing.
        #[arg(long)]
        description: Option<String>,
    },

    /// Write the site's own files from this binary.
    ///
    /// They are the loaders, components, styles, and dev-server middleware in
    /// `.vitepress/jp-vite/`, and the settings they read.
    /// The index and board pages of the RFD and ticket directories are created
    /// where they are missing, and are never changed once they exist.
    /// Lists what it will change and asks once.
    /// Refuses to overwrite a file git couldn't restore, unless `--force`.
    Sync {
        /// Write nothing, and fail when a file isn't the one sync would write.
        #[arg(long, conflicts_with_all = ["force", "yes"])]
        check: bool,

        /// Overwrite files git couldn't restore.
        #[arg(long)]
        force: bool,

        /// Write without asking.
        #[arg(long)]
        yes: bool,
    },

    /// Run the dev server, keeping the site's settings in step with the
    /// configuration.
    ///
    /// The RFD and ticket pages can be edited only here.
    /// Arguments after `--` go to `vitepress dev`.
    Serve {
        /// Arguments for `vitepress dev`, such as `--open`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        vitepress: Vec<String>,
    },
}

/// The protocol version this plugin needs from the host.
///
/// 10 is the first to read the configuration again on request (`read_config`
/// with `reload`), which is how `serve` follows a configuration that changes
/// while the site runs.
const REQUIRED_PROTOCOL: u32 = 10;

/// Where this plugin attaches to `jp`, readable from the binary without running
/// it.
///
/// `protocol` has to match `REQUIRED_PROTOCOL`.
pub static MANIFEST: &str = jp_plugin::manifest!(
    protocol: 10,
    description: "Set up, sync, and serve the site for RFDs and tickets",
    command: ["vite"],
);

/// The host a command runs under, reached over the protocol.
///
/// Every request waits for its answer before the next one goes out, so no reply
/// has to be matched to its request by id.
pub(crate) struct Session<R, W> {
    pub stdin: R,
    pub stdout: W,
}

impl<R: BufRead, W: Write> Session<R, W> {
    /// Print `text` for the user.
    fn print(&mut self, text: &str) -> Result<(), String> {
        print_to(&mut self.stdout, text, "content")
    }

    /// Ask the host to collect something from the user.
    ///
    /// `None` when there is nobody at a terminal to ask, or they declined.
    fn compose(&mut self, request: ComposeRequest) -> Result<Option<String>, String> {
        send(&mut self.stdout, &PluginToHost::Compose(request))?;

        match read_message(&mut self.stdin)? {
            HostToPlugin::Composed(response) => Ok(response.text),
            HostToPlugin::Error(error) => Err(error.message),
            HostToPlugin::Shutdown => Err("Interrupted.".to_owned()),
            other => Err(format!("expected a composed response, got: {other:?}")),
        }
    }

    /// Ask for one line of text, naming the flag that passes it instead when
    /// there is nobody to ask.
    fn ask(&mut self, message: &str, flag: &str) -> Result<String, String> {
        let answer = self.compose(ComposeRequest {
            id: None,
            message: message.to_owned(),
            mode: ComposeMode::Line { default: None },
            help: None,
        })?;

        match answer.map(|answer| answer.trim().to_owned()) {
            Some(answer) if !answer.is_empty() => Ok(answer),
            _ => Err(format!(
                "Nothing written: `jp vite init` needs an answer to \"{message}\". Answer it at a \
                 terminal, or pass `{flag}`."
            )),
        }
    }
}

/// Serve one plugin session: read the host's first message and answer it.
///
/// A `describe` is answered and ends the session; an `init` runs the command
/// its arguments name.
pub fn run(mut stdin: impl BufRead + Send + 'static, mut stdout: impl Write) -> Result<(), String> {
    match read_message(&mut stdin)? {
        HostToPlugin::Describe => send_describe(&mut stdout),
        HostToPlugin::Init(init) => {
            match jp_plugin::ready(REQUIRED_PROTOCOL, init.version) {
                Ok(ready) => send(&mut stdout, &PluginToHost::Ready(ready))?,
                Err(exit) => return send(&mut stdout, &PluginToHost::Exit(exit)),
            }

            if init.tool.is_some() {
                return send_exit(&mut stdout, 1, Some("`jp vite` serves no tools."));
            }

            handle_command(&init, Session { stdin, stdout })
        }
        other => Err(format!("expected init or describe, got: {other:?}")),
    }
}

fn handle_command<R, W>(init: &InitMessage, mut session: Session<R, W>) -> Result<(), String>
where
    R: BufRead + Send + 'static,
    W: Write,
{
    let parsed = Args::try_parse_from(iter::once("jp vite".to_owned()).chain(init.args.clone()));

    let args = match parsed {
        Ok(args) => args,
        // `--help` and `--version` surface as errors that aren't failures.
        Err(error) if !error.use_stderr() => {
            session.print(&error.to_string())?;
            return send_exit(&mut session.stdout, 0, None);
        }
        Err(error) => return send_exit(&mut session.stdout, 1, Some(&error.to_string())),
    };

    let options = match Options::read(&init.options) {
        Ok(options) => options,
        Err(message) => return send_exit(&mut session.stdout, 1, Some(&message)),
    };
    let root = init.workspace.root.as_path();

    let result = match args.command {
        Command::Init { title, description } => init_site(
            root,
            &options,
            &init.config,
            title,
            description,
            &mut session,
        ),
        Command::Sync { check, force, yes } => sync_site(
            root,
            &options,
            &init.config,
            &SyncFlags { check, force, yes },
            &mut session,
        ),
        Command::Serve { vitepress } => {
            let Session { stdin, mut stdout } = session;
            let result = serve::serve(stdin, &mut stdout, root, &options, &init.config, &vitepress);
            return finish(&mut stdout, result.map(|()| String::new()));
        }
    };

    finish(&mut session.stdout, result)
}

/// Print what a command produced and exit, or exit naming why it failed.
fn finish(stdout: &mut impl Write, result: Result<String, String>) -> Result<(), String> {
    match result {
        Ok(text) => {
            if !text.is_empty() {
                print_to(stdout, &text, "content")?;
            }
            send_exit(stdout, 0, None)
        }
        Err(message) => send_exit(stdout, 1, Some(&message)),
    }
}

/// Set up a site in the source directory, asking for its title and description
/// when a file needs them and no flag gave them.
fn init_site<R: BufRead, W: Write>(
    root: &Utf8Path,
    options: &Options,
    config: &Value,
    title: Option<String>,
    description: Option<String>,
    session: &mut Session<R, W>,
) -> Result<String, String> {
    let site_dir = source_dir(root, options)?;
    let missing = init::missing(&site_dir);
    if missing.is_empty() {
        return Ok(format!(
            "{}/ already has its package.json, configuration, theme, and homepage.\n",
            shown(root, &site_dir)
        ));
    }

    let (title, description) = if missing.iter().any(|template| template.titled) {
        let title = match title {
            Some(title) => title,
            None => session.ask("The site's title", "--title")?,
        };
        let description = match description {
            Some(description) => description,
            None => session.ask("One line describing the site", "--description")?,
        };
        (title, description)
    } else {
        (String::new(), String::new())
    };

    let written = init::write(root, &site_dir, &missing, &init::Fill {
        title,
        description,
        nav: nav(root, options, config),
    })?;

    let mut text = String::new();
    for path in written {
        text.push_str(&format!("Created {path}\n"));
    }
    text.push_str(
        "The configuration imports files `jp vite sync` writes, with the RFD and ticket pages: \
         run it next, then `jp vite serve` to see the site.\n",
    );

    Ok(text)
}

/// The navigation links to the workflows the project has: the RFD and ticket
/// indexes.
///
/// A configuration `sync` would refuse gets no links rather than no site:
/// `sync` names the problem when it is run.
fn nav(root: &Utf8Path, options: &Options, config: &Value) -> Vec<(String, String)> {
    let Ok(settings) = snapshot(root, options, config, None) else {
        return vec![];
    };

    let mut nav = vec![];
    if let Some(rfd) = &settings.rfd {
        nav.push(("RFDs".to_owned(), format!("/{}/", rfd.route)));
    }
    if let Some(ticket) = &settings.ticket {
        nav.push(("Tickets".to_owned(), format!("/{}/", ticket.route)));
    }

    nav
}

/// How `sync` was asked to run.
struct SyncFlags {
    check: bool,
    force: bool,
    yes: bool,
}

/// The value of the confirmation's choice that writes.
const WRITE: &str = "write";

/// Write the owned files, or with `check`, report the ones that differ.
fn sync_site<R: BufRead, W: Write>(
    root: &Utf8Path,
    options: &Options,
    config: &Value,
    flags: &SyncFlags,
    session: &mut Session<R, W>,
) -> Result<String, String> {
    let site_dir = source_dir(root, options)?;
    if !site_dir.is_dir() {
        return Err(format!(
            "There is no site in {}: run `jp vite init` to set one up.",
            shown(root, &site_dir)
        ));
    }

    let origin = git::origin(root);
    let settings = snapshot(root, options, config, origin.as_deref())?;
    let owned_dir = site::owned_dir(&settings);
    let changes = changes(root, &settings)?;

    if changes.is_empty() {
        return Ok("The site's files are the ones `jp vite sync` writes.\n".to_owned());
    }
    if flags.check {
        return Err(format!(
            "`jp vite sync` would make these changes:\n{}Run `jp vite sync` to make them.",
            listing(&changes)
        ));
    }

    let Some(prefix) = git::prefix(root)? else {
        return Err(
            "`jp vite sync` writes only in a git work tree, where git can restore what it \
             overwrites."
                .to_owned(),
        );
    };

    if !flags.force {
        refuse_unrecorded(root, &prefix, &changes)?;
    }

    session.print(&format!(
        "`jp vite sync` will change:\n{}",
        listing(&changes)
    ))?;
    if !flags.yes {
        match session.compose(confirmation(changes.len()))? {
            Some(answer) if answer == WRITE => {}
            Some(_) => return Ok("Nothing written.\n".to_owned()),
            None => {
                return Err(
                    "Nothing written: there is no terminal to ask on. Run it again with `--yes` \
                     to write without asking."
                        .to_owned(),
                );
            }
        }
    }

    sync::apply(root, &changes, &owned_dir)?;

    Ok(format!(
        "Synced {} {}. `git diff` shows what changed.\n",
        changes.len(),
        if changes.len() == 1 { "file" } else { "files" }
    ))
}

/// What syncing changes in the project at `root`: the owned files it rewrites
/// or removes, and the pages it creates, ordered by path.
fn changes(root: &Utf8Path, settings: &settings::Settings) -> Result<Vec<Change>, String> {
    let mut changes = sync::plan(root, site::owned(settings), &site::owned_dir(settings))?;
    changes.extend(sync::missing(root, site::pages(settings)));
    changes.sort_by(|a, b| a.path().cmp(b.path()));

    Ok(changes)
}

/// Refuse when a file a change would overwrite or remove holds something git
/// couldn't restore.
///
/// A file that doesn't exist yet risks nothing.
fn refuse_unrecorded(root: &Utf8Path, prefix: &str, changes: &[Change]) -> Result<(), String> {
    let at_risk: Vec<Utf8PathBuf> = changes
        .iter()
        .map(Change::path)
        .filter(|path| root.join(path).exists())
        .map(Utf8Path::to_path_buf)
        .collect();

    let unrecorded = git::unrecorded(root, prefix, &at_risk)?;
    if unrecorded.is_empty() {
        return Ok(());
    }

    let mut message =
        String::from("Git couldn't restore these files, so `jp vite sync` leaves them:\n");
    for (path, why) in &unrecorded {
        let why = match why {
            Unrecorded::Modified => "changed since the last commit",
            Unrecorded::Untracked => "not tracked",
        };
        message.push_str(&format!("  {path} ({why})\n"));
    }
    message.push_str("Commit them, or run `jp vite sync --force` to overwrite them anyway.");

    Err(message)
}

/// The changes, one per line.
fn listing(changes: &[Change]) -> String {
    changes
        .iter()
        .map(|change| match change {
            Change::Write(owned) => format!("  write   {}\n", owned.path),
            Change::Create(page) => format!("  create  {}\n", page.path),
            Change::Remove(path) => format!("  remove  {path}\n"),
        })
        .collect()
}

/// The question `sync` asks before changing anything.
fn confirmation(count: usize) -> ComposeRequest {
    let option = |value: &str, label: &str| ComposeOption {
        value: value.to_owned(),
        label: label.to_owned(),
    };

    ComposeRequest {
        id: None,
        message: format!(
            "Change {count} {}?",
            if count == 1 { "file" } else { "files" }
        ),
        mode: ComposeMode::Select {
            options: vec![option(WRITE, "Yes"), option("skip", "No")],
            default: Some(WRITE.to_owned()),
        },
        help: Some(
            "Git can restore them: `git diff` shows what changed, and `git checkout` puts it back."
                .to_owned(),
        ),
    }
}

/// `path` as the user reads it: relative to `root` when it's below it.
pub(crate) fn shown(root: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    path.strip_prefix(root)
        .map_or_else(|_| path.to_path_buf(), Utf8Path::to_path_buf)
}

/// The usage text, rendered from the same clap definition the commands parse.
#[must_use]
pub fn help_text() -> String {
    Args::command().render_help().to_string()
}

fn send_describe(stdout: &mut impl Write) -> Result<(), String> {
    let mut describe =
        DescribeResponse::from_manifest(MANIFEST, "vite", env!("CARGO_PKG_VERSION"), help_text())
            .map_err(|e| format!("invalid embedded manifest: {e}"))?;

    describe.author = Some("Jean Mertz <git@jeanmertz.com>".to_owned());
    describe.repository = Some("https://github.com/dcdpr/jp".to_owned());

    send(stdout, &PluginToHost::Describe(describe))
}

/// Print `text` on the host's output channel `channel`.
pub(crate) fn print_to(stdout: &mut impl Write, text: &str, channel: &str) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Print(PrintMessage {
            text: text.to_owned(),
            channel: channel.to_owned(),
            format: "plain".into(),
            language: None,
        }),
    )
}

fn send_exit(stdout: &mut impl Write, code: u8, reason: Option<&str>) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Exit(ExitMessage {
            code,
            reason: reason.map(String::from),
        }),
    )
}

pub(crate) fn read_message(stdin: &mut impl BufRead) -> Result<HostToPlugin, String> {
    let mut line = String::new();
    let read = stdin
        .read_line(&mut line)
        .map_err(|error| format!("failed to read from host: {error}"))?;

    // EOF: the host closed the pipe without answering, which is what an older
    // `jp` does when it gives up on a message it couldn't read.
    if read == 0 {
        return Err(
            "The host stopped responding. Are `jp` and `jp-vite` the same build?".to_owned(),
        );
    }

    serde_json::from_str(line.trim()).map_err(|error| format!("invalid host message: {error}"))
}

pub(crate) fn send(stdout: &mut impl Write, msg: &PluginToHost) -> Result<(), String> {
    let json = serde_json::to_string(msg).map_err(|error| format!("serialize error: {error}"))?;
    writeln!(stdout, "{json}").map_err(|error| format!("write error: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("flush error: {error}"))
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
