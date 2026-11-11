//! `jp release github`: publish releases to GitHub, as GitHub Releases.
//!
//! [`run`] serves one session with the host.
//! `publish` attaches a release's publishable build, which `jp release build`
//! keeps, and the files the `assets` setting names to the GitHub Release of the
//! release's tag, with the release's notes as its description.
//! `sync` rewrites the descriptions whose notes changed since.
//!
//! Notes are rendered from the checkout the command runs in, with `jp
//! release`'s settings, as `jp release render` renders them.
//!
//! See: `docs/rfd/123-distributing-the-jp-binary.md`, "Publishing to GitHub"

mod description;
mod github;
mod options;
mod publish;
mod sync;

use std::{
    fs,
    io::{BufRead, Write},
    iter,
};

use camino::{Utf8Path, Utf8PathBuf};
use changelog::{NoAddress, ReleaseError, cli::version as parse_version, directory::Part, release};
use clap::{CommandFactory, Parser, Subcommand};
use jp_plugin::message::{
    DescribeResponse, ExitMessage, HostToPlugin, InitMessage, PluginToHost, PrintMessage,
};
use release_build::BUILDS;
use semver::Version;

use crate::{
    description::description,
    github::Repository,
    options::{Notes, Options},
    publish::Plan,
};

#[derive(Debug, Parser)]
#[command(
    name = "jp release github",
    version,
    about = "Publish releases to GitHub, as GitHub Releases."
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Publish a release's build and notes as the GitHub Release of its tag.
    ///
    /// Attaches the archives, `checksums.txt`, and `manifest.json` of the
    /// publishable build of the release that `jp release build` keeps, and the
    /// files the `assets` setting names.
    /// The description is the release's notes, rendered from this checkout.
    /// The tag has to be on GitHub, at the commit the build is of.
    /// Publishing a release again updates its Release.
    /// The Release of the highest version is marked the latest.
    ///
    /// Reads a token from `JP_GITHUB_TOKEN` or `GITHUB_TOKEN`.
    Publish {
        /// The release, such as `0.3.0`.
        #[arg(value_parser = parse_version)]
        version: Version,
    },

    /// Rewrite the description of each Release whose notes changed.
    ///
    /// Renders the notes of each release that has a Release from this checkout,
    /// which should be `main`, and updates each description that reads
    /// differently.
    ///
    /// Reads a token from `JP_GITHUB_TOKEN` or `GITHUB_TOKEN`.
    Sync,
}

/// Serve one plugin session: read the host's first message and answer it.
///
/// A `describe` is answered and ends the session; an `init` runs the command
/// its arguments name.
pub fn run(mut stdin: impl BufRead, mut stdout: impl Write) -> Result<(), String> {
    match read_message(&mut stdin)? {
        HostToPlugin::Describe => send_describe(&mut stdout),
        HostToPlugin::Init(init) => {
            match jp_plugin::ready(REQUIRED_PROTOCOL, init.version) {
                Ok(ready) => send(&mut stdout, &PluginToHost::Ready(ready))?,
                Err(exit) => return send(&mut stdout, &PluginToHost::Exit(exit)),
            }

            if init.tool.is_some() {
                return send_exit(&mut stdout, 1, Some("`jp release github` serves no tools."));
            }

            handle_command(&init, token().as_deref(), &mut stdout)
        }
        other => Err(format!("expected init or describe, got: {other:?}")),
    }
}

/// The protocol version this plugin needs from the host, the one `jp release`
/// needs too.
const REQUIRED_PROTOCOL: u32 = 12;

/// Where this plugin attaches to `jp`, readable from the binary without running
/// it.
///
/// `protocol` has to match `REQUIRED_PROTOCOL`.
pub static MANIFEST: &str = jp_plugin::manifest!(
    protocol: 12,
    description: "Publish releases to GitHub, as GitHub Releases",
    command: ["release", "github"],
);

/// Run the command `init`'s arguments name, with the GitHub `token` the
/// environment holds.
fn handle_command(
    init: &InitMessage,
    token: Option<&str>,
    stdout: &mut impl Write,
) -> Result<(), String> {
    let parsed =
        Args::try_parse_from(iter::once("jp release github".to_owned()).chain(init.args.clone()));
    let args = match parsed {
        Ok(args) => args,
        // `--help` and `--version` surface as errors that aren't failures.
        Err(error) if !error.use_stderr() => {
            print(stdout, &error.to_string())?;
            return send_exit(stdout, 0, None);
        }
        Err(error) => return send_exit(stdout, 1, Some(&error.to_string())),
    };

    let outcome = match args.command {
        Command::Publish { version } => publish_release(init, token, &version, stdout),
        Command::Sync => sync_releases(init, token),
    };

    match outcome {
        Ok(text) => {
            print(stdout, &text)?;
            send_exit(stdout, 0, None)
        }
        Err(message) => send_exit(stdout, 1, Some(&message)),
    }
}

/// Publish `version` as the GitHub Release of its tag, and say what was done.
///
/// Each file is named on the host's chrome channel as it is attached.
fn publish_release(
    init: &InitMessage,
    token: Option<&str>,
    version: &Version,
    stdout: &mut impl Write,
) -> Result<String, String> {
    let token = token.ok_or_else(no_token)?;
    let options = Options::read(&init.options)?;
    let notes = Notes::read(&init.config)?;
    let root = init.workspace.root.as_path();
    let repository = Repository::find(root, notes.repository.as_deref())?;

    let (commit, mut uploads) =
        publish::read_build(&builds(init)?.join(version.to_string()), version)?;
    let download = repository.download(&format!("v{version}"));
    uploads.extend(publish::assets(root, &options.assets, &download)?);
    publish::distinct(&uploads)?;

    let footer = footer(root, &options)?;
    let body = describe(
        root,
        &notes,
        &options,
        footer.as_deref(),
        version,
        &download,
    )?
    .ok_or_else(|| {
        format!(
            "{} has no notes for {version}; merge its notes branch, `release-notes/v{version}`, \
             into this checkout first.",
            show(root, &resolve(root, &notes.dir))
        )
    })?;

    let client = repository.client(token)?;
    let plan = Plan {
        version: version.clone(),
        commit,
        body,
        uploads,
    };

    let published = block_on(publish::publish(&client, &repository, plan, &mut |name| {
        // A host that stopped reading fails the final print too, which reports
        // it.
        drop(send_print(
            stdout,
            &format!("Attaching {name}.\n"),
            "chrome",
            "plain",
        ));
    }))?;

    Ok(publish::summary(&published, version))
}

/// Rewrite the descriptions of the Releases whose notes changed, and say which.
fn sync_releases(init: &InitMessage, token: Option<&str>) -> Result<String, String> {
    let token = token.ok_or_else(no_token)?;
    let options = Options::read(&init.options)?;
    let notes = Notes::read(&init.config)?;
    let root = init.workspace.root.as_path();
    let repository = Repository::find(root, notes.repository.as_deref())?;
    let footer = footer(root, &options)?;

    let client = repository.client(token)?;
    let updated = block_on(sync::sync(&client, &repository, &mut |version| {
        let download = repository.download(&format!("v{version}"));
        describe(
            root,
            &notes,
            &options,
            footer.as_deref(),
            version,
            &download,
        )
    }))?;

    Ok(sync::summary(&updated))
}

/// The description of the Release of `version`, whose files GitHub serves from
/// `download`; `None` when the fragments directory has no notes for it.
fn describe(
    root: &Utf8Path,
    notes: &Notes,
    options: &Options,
    footer: Option<&str>,
    version: &Version,
    download: &str,
) -> Result<Option<String>, String> {
    let dir = resolve(root, &notes.dir);
    if !dir.join(version.to_string()).is_dir() {
        return Ok(None);
    }

    let release = release::read(
        &dir,
        &Part::Release(version.clone()),
        notes.forge,
        notes.repository.as_deref(),
        &notes.headings,
    )
    .map_err(|error| match error {
        ReleaseError::NoAddress(NoAddress(forge)) => format!(
            "The forge is `{forge}`, but git's `origin` remote has no web address for credits to \
             link to; set `plugins.command.release.options.repository` to the project's, such as \
             `https://github.com/dcdpr/jp`."
        ),
        error => format!("The notes of {version} can't be read: {error}."),
    })?;

    Ok(Some(description(
        &release,
        &notes.headings,
        version,
        options.upgrade_guide.as_deref(),
        footer,
        download,
    )))
}

/// The footer `options` names, read from the workspace at `root`.
fn footer(root: &Utf8Path, options: &Options) -> Result<Option<String>, String> {
    options
        .footer
        .as_ref()
        .map(|path| {
            fs::read_to_string(root.join(path)).map_err(|error| {
                format!(
                    "`{path}`, which `plugins.command.release-github.options.footer` names, can't \
                     be read: {error}."
                )
            })
        })
        .transpose()
}

/// Where builds of the workspace's releases are kept: `release/` in its
/// user-local storage.
fn builds(init: &InitMessage) -> Result<Utf8PathBuf, String> {
    init.paths
        .user_workspace
        .as_ref()
        .map(|storage| storage.join(BUILDS))
        .ok_or_else(|| {
            "This workspace has no user-local storage, where `jp release build` keeps builds."
                .to_owned()
        })
}

/// Run `future` to its end on a runtime of its own.
fn block_on<T>(future: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("failed to start the async runtime: {error}"))?
        .block_on(future)
}

/// The token for GitHub's API in the environment, if one is set.
fn token() -> Option<String> {
    let non_empty = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());

    non_empty("JP_GITHUB_TOKEN").or_else(|| non_empty("GITHUB_TOKEN"))
}

/// Why a command that talks to GitHub can't, without a token.
fn no_token() -> String {
    "Publishing to GitHub needs a token that can write the repository's contents, in \
     `JP_GITHUB_TOKEN` or `GITHUB_TOKEN`; `gh auth token` prints one."
        .to_owned()
}

/// Resolve a directory against the workspace root.
///
/// An absolute path is taken as given; a relative one hangs off the root.
fn resolve(root: &Utf8Path, dir: &Utf8Path) -> Utf8PathBuf {
    if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        root.join(dir)
    }
}

/// `path` as shown to the user: relative to the workspace root when it is
/// inside it.
fn show(root: &Utf8Path, path: &Utf8Path) -> String {
    path.strip_prefix(root)
        .map_or_else(|_| path.to_string(), ToString::to_string)
}

/// The usage text, rendered from the same clap definition the commands parse.
#[must_use]
pub fn help_text() -> String {
    Args::command().render_help().to_string()
}

fn send_describe(stdout: &mut impl Write) -> Result<(), String> {
    let mut describe = DescribeResponse::from_manifest(
        MANIFEST,
        "release-github",
        env!("CARGO_PKG_VERSION"),
        help_text(),
    )
    .map_err(|e| format!("invalid embedded manifest: {e}"))?;

    describe.author = Some("Jean Mertz <git@jeanmertz.com>".to_owned());
    describe.repository = Some("https://github.com/dcdpr/jp".to_owned());

    send(stdout, &PluginToHost::Describe(describe))
}

/// Print `text` on the host's content channel.
fn print(stdout: &mut impl Write, text: &str) -> Result<(), String> {
    send_print(stdout, text, "content", "plain")
}

/// Print `text`, written in `format`, on the host's `channel`.
fn send_print(
    stdout: &mut impl Write,
    text: &str,
    channel: &str,
    format: &str,
) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Print(PrintMessage {
            text: text.to_owned(),
            channel: channel.to_owned(),
            format: format.to_owned(),
            language: None,
        }),
    )
}

/// End the session with a failure, or with success when `code` is 0.
fn send_exit(stdout: &mut impl Write, code: u8, reason: Option<&str>) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Exit(ExitMessage {
            code,
            reason: reason.map(String::from),
            expected: false,
        }),
    )
}

fn read_message(stdin: &mut impl BufRead) -> Result<HostToPlugin, String> {
    let mut line = String::new();
    let read = stdin
        .read_line(&mut line)
        .map_err(|error| format!("failed to read from host: {error}"))?;

    // EOF: the host closed the pipe without answering, which is what an older
    // `jp` does when it gives up on a message it couldn't read.
    if read == 0 {
        return Err(
            "The host stopped responding. Are `jp` and `jp-release-github` the same build?"
                .to_owned(),
        );
    }

    serde_json::from_str(line.trim()).map_err(|error| format!("invalid host message: {error}"))
}

fn send(stdout: &mut impl Write, msg: &PluginToHost) -> Result<(), String> {
    let json = serde_json::to_string(msg).map_err(|error| format!("serialize error: {error}"))?;
    writeln!(stdout, "{json}").map_err(|error| format!("write error: {error}"))?;
    stdout
        .flush()
        .map_err(|error| format!("flush error: {error}"))
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
