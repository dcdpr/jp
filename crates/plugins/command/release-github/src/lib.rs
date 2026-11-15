//! `jp release github`: publish and sync GitHub Releases.
//!
//! [`run`] serves one plugin session with the host:
//!
//! - `publish` uploads a release's artifacts (validated from the build
//!   directory produced by `jp release build`) and configured `assets` to the
//!   GitHub Release for `v<version>`, setting its description to the rendered
//!   release notes.
//! - `sync` updates the descriptions of existing GitHub Releases whose release
//!   notes have changed in the current checkout.
//!
//! Release notes are rendered from the current checkout using the shared
//! `plugins.command.release.options` settings ([`release_options::Notes`]).
//!
//! See `docs/rfd/123-distributing-the-jp-binary.md` ("Publishing to GitHub").

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
use jp_plugin::{client, message::InitMessage};
use release_build::BUILDS;
use release_options::{Notes, no_address};
use semver::Version;

use crate::{description::description, github::Repository, options::Options, publish::Plan};

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
    /// Publish a release's build artifacts and notes as the GitHub Release for
    /// its tag.
    ///
    /// Uploads the archives, `checksums.txt`, and `manifest.json` from the
    /// release build stored by `jp release build`, along with any files listed
    /// in `assets`.
    /// Sets the release description to the release notes rendered from the
    /// current checkout.
    ///
    /// Requires tag `v<version>` to exist on GitHub at the commit recorded in
    /// the build manifest.
    /// Re-publishing an existing release updates it in place, and marks the
    /// highest published version as the latest release.
    ///
    /// Reads the GitHub API token from `JP_GITHUB_TOKEN` or `GITHUB_TOKEN`.
    Publish {
        /// Release version, such as `0.3.0`.
        #[arg(value_parser = parse_version)]
        version: Version,
    },

    /// Update the description of each GitHub Release whose release notes
    /// changed.
    ///
    /// Renders the notes for every published release from the current checkout
    /// (typically `main`) and updates any GitHub Release whose description
    /// differs.
    ///
    /// Reads the GitHub API token from `JP_GITHUB_TOKEN` or `GITHUB_TOKEN`.
    Sync,
}

/// Serve a single plugin session from `stdin` to `stdout`.
///
/// Answers a `describe` request immediately, or runs the subcommand specified
/// by an `init` message.
pub fn run(mut stdin: impl BufRead, mut stdout: impl Write) -> Result<(), String> {
    let describe = || {
        client::describe(
            MANIFEST,
            "release-github",
            help_text(),
            jp_plugin::package!(),
        )
    };
    let Some(init) = client::start(
        &mut stdin,
        &mut stdout,
        "jp-release-github",
        REQUIRED_PROTOCOL,
        describe,
    )?
    else {
        return Ok(());
    };
    if init.tool.is_some() {
        return client::exit(&mut stdout, 1, Some("`jp release github` serves no tools."));
    }

    handle_command(&init, token().as_deref(), &mut stdout)
}

const REQUIRED_PROTOCOL: u32 = 12;

/// Embedded plugin manifest read by `jp` without spawning the binary.
///
/// `protocol` must match `REQUIRED_PROTOCOL`.
pub static MANIFEST: &str = jp_plugin::manifest!(
    protocol: 12,
    description: "Publish releases to GitHub, as GitHub Releases",
    command: ["release", "github"],
);

fn handle_command(
    init: &InitMessage,
    token: Option<&str>,
    stdout: &mut impl Write,
) -> Result<(), String> {
    let parsed =
        Args::try_parse_from(iter::once("jp release github".to_owned()).chain(init.args.clone()));
    let args = match parsed {
        Ok(args) => args,
        // `--help` and `--version` exit 0 via clap's non-stderr display path.
        Err(error) if !error.use_stderr() => {
            client::print(stdout, &error.to_string(), "content", "plain")?;
            return client::exit(stdout, 0, None);
        }
        Err(error) => return client::exit(stdout, 1, Some(&error.to_string())),
    };

    let outcome = match args.command {
        Command::Publish { version } => publish_release(init, token, &version, stdout),
        Command::Sync => sync_releases(init, token),
    };

    match outcome {
        Ok(text) => {
            client::print(stdout, &text, "content", "plain")?;
            client::exit(stdout, 0, None)
        }
        Err(message) => client::exit(stdout, 1, Some(&message)),
    }
}

fn publish_release(
    init: &InitMessage,
    token: Option<&str>,
    version: &Version,
    stdout: &mut impl Write,
) -> Result<String, String> {
    let token = token.ok_or_else(no_token)?;
    let options = Options::read(&init.options)?;
    let notes = Notes::from_config(&init.config)?;
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
            show(root, &resolve(root, &notes.fragments_dir))
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
        drop(client::print(
            stdout,
            &format!("Attaching {name}.\n"),
            "chrome",
            "plain",
        ));
    }))?;

    Ok(publish::summary(&published, version))
}

fn sync_releases(init: &InitMessage, token: Option<&str>) -> Result<String, String> {
    let token = token.ok_or_else(no_token)?;
    let options = Options::read(&init.options)?;
    let notes = Notes::from_config(&init.config)?;
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

/// Render the GitHub Release description for `version`, or return `None` if the
/// fragments directory has no `<version>/` subdirectory.
fn describe(
    root: &Utf8Path,
    notes: &Notes,
    options: &Options,
    footer: Option<&str>,
    version: &Version,
    download: &str,
) -> Result<Option<String>, String> {
    let dir = resolve(root, &notes.fragments_dir);
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
        ReleaseError::NoAddress(NoAddress(forge)) => no_address(forge),
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

fn block_on<T>(future: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("failed to start the async runtime: {error}"))?
        .block_on(future)
}

fn token() -> Option<String> {
    let non_empty = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());

    non_empty("JP_GITHUB_TOKEN").or_else(|| non_empty("GITHUB_TOKEN"))
}

fn no_token() -> String {
    "Publishing to GitHub needs a token that can write the repository's contents, in \
     `JP_GITHUB_TOKEN` or `GITHUB_TOKEN`; `gh auth token` prints one."
        .to_owned()
}

fn resolve(root: &Utf8Path, dir: &Utf8Path) -> Utf8PathBuf {
    if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        root.join(dir)
    }
}

fn show(root: &Utf8Path, path: &Utf8Path) -> String {
    path.strip_prefix(root)
        .map_or_else(|_| path.to_string(), ToString::to_string)
}

/// Render the top-level CLI `--help` text.
#[must_use]
pub fn help_text() -> String {
    Args::command().render_help().to_string()
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
