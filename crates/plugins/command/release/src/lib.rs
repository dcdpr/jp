//! `jp release` command plugin: recording changelog fragments and highlights,
//! and managing the release lifecycle.
//!
//! This crate provides the `jp release` subcommands and the `changelog_add`
//! tool (`source = "plugin.command.release.add"`).
//! When `init` carries a tool call, the `tool` module handles it; otherwise the
//! CLI subcommand in `init.args` runs.
//!
//! Parsing, directory validation, and release-note rendering live in the
//! [`changelog`] crate.
//! Within this crate:
//!
//! - `edit` opens fragments and highlights in the user's editor via the host.
//! - `entry` derives file names and writes validated fragments (formatting them
//!   with `.comfort.toml` when `format_on_write` is enabled).
//! - `options` parses `plugins.command.release.options`.
//! - `definitions` scaffolds the `changelog_add` tool definition.
//! - `cut`, `pick`, `finalize`, `build`, `abandon`, `due`, and `status`
//!   implement the release pipeline over `releases` and `repository`.
//!
//! See `docs/rfd/121-changelog-fragments-and-validation.md` and
//! `docs/rfd/122-release-pipeline.md`.

mod abandon;
mod build;
mod cut;
mod definitions;
mod due;
mod edit;
mod entry;
mod finalize;
mod options;
mod pick;
mod pipeline;
mod releases;
mod repository;
#[cfg(test)]
mod repository_testing;
mod shell;
mod status;
mod tool;

use std::{
    env,
    fs::{self, OpenOptions},
    io::{BufRead, Write},
    iter,
    process::Command as Process,
    time::{SystemTime, UNIX_EPOCH},
};

use camino::{Utf8Path, Utf8PathBuf};
use changelog::{
    NoAddress, ReleaseError, Type,
    cli::{Format, Render, View, version as parse_version},
    directory::ReadError,
    release,
};
use clap::{CommandFactory, Parser, Subcommand};
use jp_plugin::{
    client,
    message::{InitMessage, OutputFormat, ToolCall},
};
use release_build::{BUILDS, Profile};
use release_options::no_address;
use semver::Version;
use serde_json::Value;

use crate::{
    edit::Host,
    entry::{Fragments, HIGHLIGHTS},
    options::Options,
    pipeline::Pipeline,
    repository::MAIN,
    shell::Shell,
};

pub(crate) const PLUGIN: &str = "release";

const README: &str = include_str!("../templates/README.md");
const README_FILE: &str = "README.md";

#[derive(Debug, Parser)]
#[command(
    name = "jp release",
    version,
    about = "Record what changes for users as changelog fragments and highlights, and make \
             releases from them."
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create the fragments directory, its `highlights/`, and a README
    /// describing both formats.
    ///
    /// What is already there is left as it is.
    Init,

    #[command(flatten)]
    Record(Record),

    /// Write or edit a highlight, a section of the release notes for a feature
    /// worth a story, in your editor.
    ///
    /// A new highlight opens with a `# Title` to write, which names its file,
    /// and you credited as its author.
    /// Saving it empty writes nothing.
    Highlight {
        /// The highlight's name: `highlights/<slug>.md` in the fragments
        /// directory, which opens for another edit when it is there.
        ///
        /// Without it, a new highlight is named after its title.
        slug: Option<String>,
    },

    /// Check every file in the fragments directory, released ones included, and
    /// exit 1 when one breaks a rule.
    Check,

    /// Print a release's notes or upgrade guide, or the fragments no release
    /// has taken yet.
    ///
    /// Each fragment and highlight is credited to the change requests that
    /// wrote it, read from git's history, so a shallow clone is refused.
    /// `--format` defaults to `json` when `jp` prints JSON.
    Render {
        #[command(flatten)]
        render: Render,
    },

    /// Start the next minor or major release from `main`.
    ///
    /// `main` is the commit you have checked out, which needs every commit of
    /// origin's `main`.
    /// Branches `release/<major>.<minor>` from it, with a commit that removes
    /// the fragments directory and runs `bump`, and `release-notes/v<version>`,
    /// with one that moves every unreleased fragment and highlight into the
    /// release's directory.
    /// Pushes nothing: it prints the push that publishes both, and merging the
    /// notes branch into `main` files the release's notes.
    Cut {
        /// The version to start, such as `0.4.0`, rather than the next minor
        /// release, or from 1.0 on the next major one when a fragment has an
        /// upgrade section.
        #[arg(long, value_parser = parse_version)]
        version: Option<Version>,

        /// Start the release branch at this commit, an earlier one on the
        /// first-parent line of the commit checked out, to release the history
        /// up to it.
        ///
        /// Only the fragments whose changes it holds are filed: those whose
        /// `Credits` name its commits, or that it added itself.
        #[arg(long, value_name = "COMMIT")]
        at: Option<String>,
    },

    /// Pick commits from `main` into a release in preparation, or start the
    /// next patch release with them.
    ///
    /// Cherry-picks the commits, from the first-parent line of the commit you
    /// have checked out, onto the release branch, leaving out the fragments
    /// directory.
    /// Files the fragments and highlights they added into the release's
    /// directory on `release-notes/v<version>`, merging `main` into it first.
    /// Pushes nothing: it prints the push that publishes both.
    /// A conflict stops the pick in a worktree it names; resolve it there and
    /// run `jp release pick --continue`.
    Pick {
        /// The release, such as `0.3.1`.
        #[arg(value_parser = parse_version, required_unless_present = "resume")]
        version: Option<Version>,

        /// The commits to pick, from `main`.
        #[arg(value_name = "COMMIT", required_unless_present = "resume")]
        commits: Vec<String>,

        /// Finish the pick a conflict stopped, once it is resolved.
        #[arg(long = "continue", conflicts_with_all = ["version", "commits"])]
        resume: bool,
    },

    /// Tag a release in preparation: the tip of its release branch, as
    /// `v<version>`, with its release notes and upgrade guide as the tag's
    /// message.
    ///
    /// The notes are read from the commit checked out, so merge the release's
    /// notes branch into `main` first.
    /// Pushes nothing: it prints the push that publishes the tag, which makes
    /// the release.
    Finalize {
        /// The release, such as `0.3.0`.
        #[arg(value_parser = parse_version)]
        version: Version,
    },

    /// Build a release's binaries with `plugins.command.release.options.build`,
    /// and keep them in this workspace's user-local storage.
    ///
    /// Builds the commit the release's tag names, or while the release is in
    /// preparation, the tip of its release branch, in a temporary worktree.
    /// Checks what the build wrote, its archives, `checksums.txt`, and
    /// `manifest.json`, then keeps it in `release/<version>/` of `jp path
    /// user-workspace`, replacing an earlier build of that version.
    /// Publishers take only a build made with the release profile from the
    /// commit the release's tag names.
    #[command(after_long_help = build::CONTRACT)]
    Build {
        /// The release, such as `0.3.0`.
        #[arg(value_parser = parse_version)]
        version: Version,

        /// Build with the quick profile, to test the build in less time;
        /// publishers refuse quick builds.
        #[arg(long)]
        quick: bool,
    },

    /// Take back a release nobody has published, so it can be cut again.
    ///
    /// Refuses a release origin has tagged, and one older than another release:
    /// releases are abandoned newest first.
    /// Deletes its tag and notes branch here, and its release branch, or for a
    /// patch release the commits from the one that started it.
    /// Moves the fragments and highlights its notes filed back to the top of
    /// the fragments directory on `release-notes/abandon-v<version>`, with
    /// their wording as edited, for merging into `main`.
    /// Pushes nothing: it prints the pushes that take the release back on
    /// origin.
    Abandon {
        /// The release, such as `0.3.0`.
        #[arg(value_parser = parse_version)]
        version: Version,
    },

    /// Exit 0 when the next release should be cut, and 1 when it shouldn't,
    /// saying why either way.
    ///
    /// The next release is due once `cadence` has passed since the newest minor
    /// or major release was cut, that release has been finalized, and `main`
    /// has an unreleased fragment to release.
    /// With `jp --format json`, prints `{"due": <bool>, "reason": <text>}`.
    Due,

    /// Say where the releases stand, and what to run next.
    ///
    /// Reads the release branches, notes branches, and tags here and on
    /// `origin`, and the commit checked out, which stands for `main`, as `cut`,
    /// `pick`, and `finalize` do.
    /// Changes nothing.
    Status,

    /// Write the definition of the tool that offers `add` to the assistant.
    Tools {
        #[command(subcommand)]
        command: ToolsCommand,
    },
}

/// Fragment-recording subcommands, one per [`Type`].
#[derive(Debug, Subcommand)]
enum Record {
    /// Write or edit an `added` fragment (a new user-facing capability) in your
    /// editor.
    Added(Name),

    /// Write or edit a `changed` fragment (a change to existing behavior) in
    /// your editor.
    Changed(Name),

    /// Write or edit a `deprecated` fragment (a feature slated for removal) in
    /// your editor.
    Deprecated(Name),

    /// Write or edit a `removed` fragment (a removed feature or option) in your
    /// editor.
    Removed(Name),

    /// Write or edit a `fixed` fragment (a user-visible bug fix) in your
    /// editor.
    Fixed(Name),

    /// Write or edit a `security` fragment (a security-relevant fix) in your
    /// editor.
    Security(Name),

    /// Write or edit a `performance` fragment (a user-visible performance
    /// improvement) in your editor.
    Performance(Name),
}

impl Record {
    fn split(self) -> (Type, Option<String>) {
        match self {
            Self::Added(name) => (Type::Added, name.slug),
            Self::Changed(name) => (Type::Changed, name.slug),
            Self::Deprecated(name) => (Type::Deprecated, name.slug),
            Self::Removed(name) => (Type::Removed, name.slug),
            Self::Fixed(name) => (Type::Fixed, name.slug),
            Self::Security(name) => (Type::Security, name.slug),
            Self::Performance(name) => (Type::Performance, name.slug),
        }
    }
}

#[derive(Debug, clap::Args)]
struct Name {
    /// Fragment slug (`<slug>.<type>.md` in the fragments directory).
    ///
    /// Defaults to the last `/`-separated segment of the current git branch.
    /// Saving an empty file removes the fragment.
    slug: Option<String>,
}

#[derive(Debug, Subcommand)]
enum ToolsCommand {
    /// Write the default `changelog_add` tool definition.
    ///
    /// Files that already match the shipped definition are left untouched.
    /// Modified files are overwritten if git can restore them; otherwise
    /// `--force` is required.
    Init {
        /// Output directory, relative to the workspace root.
        path: Utf8PathBuf,

        /// Overwrite existing files that git cannot restore.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Default)]
pub(crate) struct Output {
    text: String,

    /// Exit code (`0` by default; `1` when `check` finds problems or `due`
    /// reports that no release is due).
    code: u8,

    /// Whether `text` is JSON.
    json: bool,

    /// Optional status note printed on the `chrome` channel (stderr).
    note: Option<String>,
}

impl From<String> for Output {
    fn from(text: String) -> Self {
        Self {
            text,
            ..Self::default()
        }
    }
}

struct Context<'a> {
    fragments: Fragments<'a>,
    config: &'a Value,
    options: &'a Options,
    format: OutputFormat,
}

/// Serve a single plugin session from `stdin` to `stdout`.
///
/// Answers a `describe` request immediately, dispatches tool calls to the
/// `tool` module, or runs the CLI subcommand from `init.args`.
pub fn run(mut stdin: impl BufRead, mut stdout: impl Write) -> Result<(), String> {
    let describe = || client::describe(MANIFEST, "release", help_text(), jp_plugin::package!());
    let Some(init) = client::start(&mut stdin, &mut stdout, BINARY, REQUIRED_PROTOCOL, describe)?
    else {
        return Ok(());
    };

    match &init.tool {
        Some(call) => answer_tool_call(&init, call, &mut stdout),
        None => handle_command(&init, &mut stdin, &mut stdout),
    }
}

pub(crate) const BINARY: &str = "jp-release";

/// Minimum host protocol version required by this plugin (v12 introduced `tool`
/// on `init` and `tool_outcome`).
const REQUIRED_PROTOCOL: u32 = 12;

/// Embedded plugin manifest read by `jp` without spawning the binary.
///
/// `protocol` must match `REQUIRED_PROTOCOL`.
pub static MANIFEST: &str = jp_plugin::manifest!(
    protocol: 12,
    description: "Record what changes for users as changelog fragments, and make releases",
    command: ["release"],
);

/// Execute the tool call in `init` and write its `tool_outcome` message.
///
/// Invalid plugin options fail the plugin process directly (`exit(1)`) rather
/// than returning a tool error outcome, because the assistant cannot fix
/// workspace configuration.
fn answer_tool_call(
    init: &InitMessage,
    call: &ToolCall,
    stdout: &mut impl Write,
) -> Result<(), String> {
    let options = match Options::read(&init.options) {
        Ok(options) => options,
        Err(message) => return client::exit(stdout, 1, Some(&message)),
    };

    client::answer_tool(stdout, &tool::run(init, call, &options))
}

fn handle_command(
    init: &InitMessage,
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
) -> Result<(), String> {
    let parsed = Args::try_parse_from(iter::once("jp release".to_owned()).chain(init.args.clone()));
    let args = match parsed {
        Ok(args) => args,
        // `--help` and `--version` exit 0 via clap's non-stderr display path.
        Err(error) if !error.use_stderr() => {
            client::print(stdout, &error.to_string(), "content", "plain")?;
            return client::exit(stdout, 0, None);
        }
        Err(error) => return client::exit(stdout, 1, Some(&error.to_string())),
    };

    let options = match Options::read(&init.options) {
        Ok(options) => options,
        Err(message) => return client::exit(stdout, 1, Some(&message)),
    };
    let ctx = match Context::new(init, &options) {
        Ok(ctx) => ctx,
        Err(message) => return client::exit(stdout, 1, Some(&message)),
    };

    let result = dispatch(&ctx, init, args.command, stdin, stdout);
    finish(stdout, result)
}

impl<'a> Context<'a> {
    fn new(init: &'a InitMessage, options: &'a Options) -> Result<Self, String> {
        let root = init.workspace.root.as_path();
        let dir = resolve_dir(root, &options.notes.fragments_dir);

        Ok(Self {
            fragments: Fragments::new(root, dir, &init.config)?,
            config: &init.config,
            options,
            format: init.output_format,
        })
    }
}

fn dispatch(
    ctx: &Context<'_>,
    init: &InitMessage,
    command: Command,
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
) -> Result<Output, String> {
    let root = ctx.fragments.root;
    let options = ctx.options;

    match command {
        Command::Init => init_dir(ctx),
        Command::Record(record) => {
            let (type_, slug) = record.split();
            edit_fragment(ctx, type_, slug, init.version, stdin, stdout)
        }
        Command::Highlight { slug } => {
            edit_highlight(ctx, slug.as_deref(), init.version, stdin, stdout)
        }
        Command::Check => check(ctx),
        Command::Render { render } => render_release(ctx, &render),
        Command::Cut { version, at } => make_release(root, options, |pipeline| {
            cut::cut(pipeline, version.as_ref(), at.as_deref())
        }),
        Command::Pick {
            version: Some(version),
            commits,
            resume: false,
        } => make_release(root, options, |pipeline| {
            pick::pick(pipeline, &version, &commits)
        }),
        Command::Pick { .. } => make_release(root, options, pick::resume),
        Command::Finalize { version } => make_release(root, options, |pipeline| {
            finalize::finalize(pipeline, &version)
        }),
        Command::Build { version, quick } => build_release(ctx, init, &version, quick, stdout),
        Command::Abandon { version } => make_release(root, options, |pipeline| {
            abandon::abandon(pipeline, &version)
        }),
        Command::Due => due_release(root, options, ctx.format),
        Command::Status => make_release(root, options, |pipeline| {
            status::status(pipeline, now()?, builds(init).as_deref())
        }),
        Command::Tools {
            command: ToolsCommand::Init { path, force },
        } => definitions::init(&resolve_dir(root, &path), force),
    }
}

fn finish(stdout: &mut impl Write, result: Result<Output, String>) -> Result<(), String> {
    let output = match result {
        Ok(output) => output,
        Err(message) => return client::exit(stdout, 1, Some(&message)),
    };

    if let Some(note) = &output.note {
        client::print(stdout, note, "chrome", "plain")?;
    }
    if !output.text.is_empty() {
        let format = if output.json { "json" } else { "plain" };
        client::print(stdout, &output.text, "content", format)?;
    }

    client::result(stdout, output.code)
}

fn build_release(
    ctx: &Context<'_>,
    init: &InitMessage,
    version: &Version,
    quick: bool,
    stdout: &mut impl Write,
) -> Result<Output, String> {
    let profile = if quick {
        Profile::Quick
    } else {
        Profile::Release
    };
    let builds = builds(init).ok_or_else(|| {
        "This workspace has no user-local storage, where `jp release build` keeps builds."
            .to_owned()
    })?;

    make_release(ctx.fragments.root, ctx.options, |pipeline| {
        build::build(pipeline, &builds, version, profile, &mut |line| {
            client::print(stdout, &format!("{line}\n"), "chrome", "plain")
        })
    })
}

/// Create `<fragments_dir>/`, `<fragments_dir>/highlights/`, and
/// `<fragments_dir>/README.md` if they do not already exist.
fn init_dir(ctx: &Context<'_>) -> Result<Output, String> {
    let fragments = &ctx.fragments;
    let mut text = String::new();

    for dir in [fragments.dir.clone(), fragments.dir.join(HIGHLIGHTS)] {
        if dir.is_dir() {
            continue;
        }
        fs::create_dir_all(&dir).map_err(|error| format!("failed to create {dir}: {error}"))?;
        text.push_str(&format!("Created {}/\n", fragments.show(&dir)));
    }

    let readme = fragments.dir.join(README_FILE);
    if !readme.exists() {
        let contents = fragments.formatting.apply(&readme, README);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&readme)
            .and_then(|mut file| file.write_all(contents.as_bytes()))
            .map_err(|error| format!("failed to write {readme}: {error}"))?;
        text.push_str(&format!("Wrote {}\n", fragments.show(&readme)));
    }

    if text.is_empty() {
        text = format!(
            "{}/ already has its `highlights/` and its README.\n",
            fragments.show(&fragments.dir)
        );
    }

    Ok(text.into())
}

fn edit_fragment(
    ctx: &Context<'_>,
    type_: Type,
    slug: Option<String>,
    version: u32,
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
) -> Result<Output, String> {
    edit::check_host(version)?;
    let slug = match slug {
        Some(slug) => slug,
        None => branch_slug(ctx.fragments.root, type_)?,
    };

    edit::fragment(&ctx.fragments, type_, &slug, &mut Host { stdin, stdout })
}

fn edit_highlight(
    ctx: &Context<'_>,
    slug: Option<&str>,
    version: u32,
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
) -> Result<Output, String> {
    edit::check_host(version)?;
    let author = identity(ctx.fragments.root, ctx.config);

    edit::highlight(&ctx.fragments, slug, author.as_deref(), &mut Host {
        stdin,
        stdout,
    })
}

/// Derive a default fragment slug from the current git branch in `root`.
fn branch_slug(root: &Utf8Path, type_: Type) -> Result<String, String> {
    let name_it = format!("name it: `jp release {type_} <slug>`");
    let branch = Process::new("git")
        .current_dir(root)
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|branch| branch.trim().to_owned());
    let Some(branch) = branch else {
        return Err(format!(
            "There is no git branch to name the fragment after; {name_it}."
        ));
    };

    entry::branch_slug(&branch).ok_or_else(|| {
        if branch == MAIN {
            format!("`{MAIN}` has no change of its own to name the fragment after; {name_it}.")
        } else {
            format!("The branch `{branch}` holds no word to name the fragment after; {name_it}.")
        }
    })
}

/// Resolve the default author for a new highlight from JP's `user.name` or `git
/// config user.name`.
fn identity(root: &Utf8Path, config: &Value) -> Option<String> {
    let configured = config
        .pointer("/user/name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty());
    if let Some(name) = configured {
        return Some(name.to_owned());
    }

    Process::new("git")
        .current_dir(root)
        .args(["config", "--get", "user.name"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}

fn check(ctx: &Context<'_>) -> Result<Output, String> {
    let fragments = &ctx.fragments;
    existing(fragments)?;

    let mut report = changelog::check(&fragments.dir).map_err(|error| error.to_string())?;
    for problem in &mut report.problems {
        problem.path = Utf8PathBuf::from(fragments.show(&problem.path));
    }

    Ok(Output {
        text: format!("{report}\n"),
        code: u8::from(!report.problems.is_empty()),
        ..Output::default()
    })
}

fn render_release(ctx: &Context<'_>, render: &Render) -> Result<Output, String> {
    let default = if ctx.format.is_json() {
        Format::Json
    } else {
        Format::Markdown
    };
    let view = render.view(default)?;

    let fragments = &ctx.fragments;
    existing(fragments)?;
    let release = release::read(
        &fragments.dir,
        &render.part(),
        ctx.options.notes.forge,
        ctx.options.notes.repository.as_deref(),
        &ctx.options.notes.headings,
    )
    .map_err(|error| unreadable(fragments, error))?;
    let text = view.render(&release, &ctx.options.notes.headings)?;

    let no_guide = text.is_empty() && view == View::UpgradeGuide;
    let note = no_guide.then(|| match &render.version {
        Some(version) => {
            format!("{version} has no upgrade guide: none of its entries has an upgrade section.\n")
        }
        None => "The unreleased fragments have no upgrade guide: none has an upgrade section.\n"
            .to_owned(),
    });

    Ok(Output {
        json: view == View::Json,
        text,
        note,
        code: 0,
    })
}

fn due_release(root: &Utf8Path, options: &Options, format: OutputFormat) -> Result<Output, String> {
    let shell = Shell::new();
    let pipeline = Pipeline::new(&shell, root, options)?;
    let due = due::due(&pipeline, now()?)?;

    let text = if format.is_json() {
        serde_json::to_string(&due)
            .map_err(|error| format!("failed to write the answer as JSON: {error}"))?
    } else {
        due.reason.clone()
    };

    Ok(Output {
        text: text + "\n",
        code: u8::from(!due.due),
        json: format.is_json(),
        note: None,
    })
}

fn builds(init: &InitMessage) -> Option<Utf8PathBuf> {
    init.paths
        .user_workspace
        .as_ref()
        .map(|storage| storage.join(BUILDS))
}

fn now() -> Result<i64, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("The system clock is set before 1970: {error}."))?;

    Ok(i64::try_from(now.as_secs()).unwrap_or(i64::MAX))
}

fn make_release(
    root: &Utf8Path,
    options: &Options,
    command: impl FnOnce(&Pipeline<'_>) -> Result<String, String>,
) -> Result<Output, String> {
    let shell = Shell::new();
    let pipeline = Pipeline::new(&shell, root, options)?;

    command(&pipeline).map(Output::from)
}

fn existing(fragments: &Fragments<'_>) -> Result<(), String> {
    if fragments.dir.is_dir() {
        return Ok(());
    }

    Err(format!(
        "There is no fragments directory at {}; create it with `jp release init`.",
        fragments.show(&fragments.dir)
    ))
}

fn unreadable(fragments: &Fragments<'_>, error: ReleaseError) -> String {
    let shown = |path: &Utf8Path| Utf8PathBuf::from(fragments.show(path));

    match error {
        ReleaseError::Read(ReadError::Problems(mut problems)) => {
            for problem in &mut problems {
                problem.path = shown(&problem.path);
            }
            format!(
                "Fix these first, which `jp release check` reports too:\n{}",
                ReadError::Problems(problems)
            )
        }
        ReleaseError::Read(ReadError::NoRelease { dir, version }) => ReadError::NoRelease {
            dir: shown(&dir),
            version,
        }
        .to_string(),
        ReleaseError::NoAddress(NoAddress(forge)) => no_address(forge),
        error => error.to_string(),
    }
}

fn resolve_dir(root: &Utf8Path, dir: &Utf8Path) -> Utf8PathBuf {
    if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        root.join(dir)
    }
}

/// Render the top-level CLI `--help` text.
#[must_use]
pub fn help_text() -> String {
    Args::command().render_help().to_string()
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
