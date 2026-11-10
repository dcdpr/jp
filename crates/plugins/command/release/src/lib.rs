//! `jp-release`: record what changes for users as changelog fragments and
//! highlights, and make releases from them.
//!
//! A command plugin providing `jp release` and its subcommands, and the tool
//! the assistant reaches through `source = "plugin.command.release.add"`.
//! An `init` carrying a tool call is answered by the `tool` module; any other
//! runs the command its arguments name.
//!
//! The formats, the check that holds a fragments directory to them, and the
//! release notes and upgrade guide rendered from them live in the `changelog`
//! crate; this crate is the commands, their configuration, and the tool over
//! it.
//! `edit` opens a fragment or highlight in the user's editor, `entry` names the
//! files and writes the fragment the tool gives, `options` reads the settings
//! from `plugins.command.release.options`, and `definitions` writes the tool's
//! definition.
//! With `format_on_write` on, every file written in the fragments directory is
//! laid out in the style of the workspace's `.comfort.toml` (`doc::format`).
//!
//! `cut`, `pick`, and `finalize` make releases on release branches and their
//! notes on notes branches, in this repository, and print the push that
//! publishes them; `build` builds a release's binaries with the project's
//! command and keeps them in the workspace's user-local storage; `abandon`
//! takes back a release nobody has published, and `due` says when the next
//! release should be cut.
//! `releases` decides what they may do, and `repository` runs the git they need
//! in worktrees of their own, reading what this repository and `origin` hold.
//!
//! See: `docs/rfd/121-changelog-fragments-and-validation.md` and
//! `docs/rfd/122-release-pipeline.md`

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
use jp_plugin::message::{
    ComposeRequest, DescribeResponse, ExitMessage, HostToPlugin, InitMessage, OutputFormat,
    PluginToHost, PrintMessage, ToolCall, ToolOutcomeMessage,
};
use semver::Version;
use serde_json::Value;

use crate::{
    build::Profile,
    edit::Host,
    entry::{Fragments, HIGHLIGHTS},
    options::{Options, no_address},
    pipeline::Pipeline,
    repository::MAIN,
    shell::Shell,
};

/// The name the plugin is configured under, `plugins.command.release`.
pub(crate) const PLUGIN: &str = "release";

/// The README `init` writes into the fragments directory, describing both
/// formats.
const README: &str = include_str!("../templates/README.md");

/// The name of the file [`README`] is written to.
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

/// A fragment of each type, written or edited in the user's editor.
#[derive(Debug, Subcommand)]
enum Record {
    /// Write or edit an `added` fragment, a new capability users can now reach,
    /// in your editor.
    Added(Name),

    /// Write or edit a `changed` fragment, a change to existing behavior, in
    /// your editor.
    Changed(Name),

    /// Write or edit a `deprecated` fragment, something still working but
    /// slated for removal, in your editor.
    Deprecated(Name),

    /// Write or edit a `removed` fragment, something taken away, in your
    /// editor.
    Removed(Name),

    /// Write or edit a `fixed` fragment, a bug fix users would notice, in your
    /// editor.
    Fixed(Name),

    /// Write or edit a `security` fragment, a fix with security relevance, in
    /// your editor.
    Security(Name),

    /// Write or edit a `performance` fragment, a performance improvement users
    /// would notice, in your editor.
    Performance(Name),
}

impl Record {
    /// The type of the fragment, and the slug given for it.
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

/// The name of a fragment to write or edit.
#[derive(Debug, clap::Args)]
struct Name {
    /// The fragment's name: `<slug>.<type>.md` in the fragments directory,
    /// which opens for another edit when it is there.
    ///
    /// Defaults to the current git branch's name, the part after its last `/`.
    /// Saving a fragment empty removes it.
    slug: Option<String>,
}

#[derive(Debug, Subcommand)]
enum ToolsCommand {
    /// Write the tool's starting definition.
    ///
    /// An existing file that matches the shipped definition is left untouched.
    /// One with different contents is overwritten if it is tracked by git with
    /// no uncommitted changes; otherwise `--force` is required.
    /// The workspace configuration has to load the directory for the tool to
    /// appear.
    Init {
        /// Directory to write it into, relative to the workspace root.
        path: Utf8PathBuf,

        /// Overwrite a file that git cannot restore.
        #[arg(long)]
        force: bool,
    },
}

/// What a subcommand produced.
#[derive(Debug, Default)]
pub(crate) struct Output {
    text: String,

    /// The exit code, for a command that reports a result by it: `check` exits
    /// 1 when it finds a problem, after printing what it found, and `due` when
    /// no release is due, after printing why.
    code: u8,

    /// Whether `text` is JSON, which the host prints as such.
    json: bool,

    /// What to tell the user beside the output, on stderr, such as why there is
    /// none.
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

/// What a command runs against.
struct Context<'a> {
    /// The fragments directory.
    fragments: Fragments<'a>,

    /// The configuration the host resolved, which names who a highlight is
    /// credited to by default.
    config: &'a Value,

    options: &'a Options,

    /// How the host prints, which decides the format `render` prints in by
    /// default.
    format: OutputFormat,
}

/// Serve one plugin session: read the host's first message and answer it.
///
/// A `describe` is answered and ends the session.
/// An `init` carrying a tool call is answered with the call's outcome; any
/// other `init` runs the command its arguments name.
pub fn run(mut stdin: impl BufRead, mut stdout: impl Write) -> Result<(), String> {
    match read_message(&mut stdin)? {
        HostToPlugin::Describe => send_describe(&mut stdout),
        HostToPlugin::Init(init) => {
            match jp_plugin::ready(REQUIRED_PROTOCOL, init.version) {
                Ok(ready) => send(&mut stdout, &PluginToHost::Ready(ready))?,
                Err(exit) => return send(&mut stdout, &PluginToHost::Exit(exit)),
            }

            match &init.tool {
                Some(call) => answer_tool_call(&init, call, &mut stdout),
                None => handle_command(&init, &mut stdin, &mut stdout),
            }
        }
        other => Err(format!("expected init or describe, got: {other:?}")),
    }
}

/// The protocol version this plugin needs from the host.
///
/// 12 is the first to carry tool calls (`tool` on `init`, answered with
/// `tool_outcome`), which is how `changelog_add` reaches this plugin.
const REQUIRED_PROTOCOL: u32 = 12;

/// Where this plugin attaches to `jp`, readable from the binary without running
/// it.
///
/// `protocol` has to match `REQUIRED_PROTOCOL`.
pub static MANIFEST: &str = jp_plugin::manifest!(
    protocol: 12,
    description: "Record what changes for users as changelog fragments, and make releases",
    command: ["release"],
);

/// Answer the tool call `init` carried, then exit.
///
/// The host closed stdin after `init`, so nothing here may ask it anything.
/// Options the plugin can't read end the call as a failed plugin run rather
/// than as an outcome: the assistant can't fix the configuration.
fn answer_tool_call(
    init: &InitMessage,
    call: &ToolCall,
    stdout: &mut impl Write,
) -> Result<(), String> {
    let options = match Options::read(&init.options) {
        Ok(options) => options,
        Err(message) => return send_exit(stdout, 1, Some(&message)),
    };

    let outcome = serde_json::to_value(tool::run(init, call, &options))
        .map_err(|error| format!("failed to serialize the tool outcome: {error}"))?;
    send(
        stdout,
        &PluginToHost::ToolOutcome(ToolOutcomeMessage { outcome }),
    )?;

    send_exit(stdout, 0, None)
}

fn handle_command(
    init: &InitMessage,
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
) -> Result<(), String> {
    let parsed = Args::try_parse_from(iter::once("jp release".to_owned()).chain(init.args.clone()));
    let args = match parsed {
        Ok(args) => args,
        // `--help` and `--version` surface as errors that aren't failures.
        Err(error) if !error.use_stderr() => {
            print(stdout, &error.to_string())?;
            return send_exit(stdout, 0, None);
        }
        Err(error) => return send_exit(stdout, 1, Some(&error.to_string())),
    };

    let options = match Options::read(&init.options) {
        Ok(options) => options,
        Err(message) => return send_exit(stdout, 1, Some(&message)),
    };
    let root = init.workspace.root.as_path();
    let dir = resolve_dir(root, &options.fragments_dir);
    let fragments = match Fragments::new(root, dir, &init.config) {
        Ok(fragments) => fragments,
        Err(message) => return send_exit(stdout, 1, Some(&message)),
    };
    let ctx = Context {
        fragments,
        config: &init.config,
        options: &options,
        format: init.output_format,
    };

    let result = match args.command {
        Command::Init => init_dir(&ctx),
        Command::Record(record) => {
            let (type_, slug) = record.split();
            edit_fragment(&ctx, type_, slug, init.version, stdin, stdout)
        }
        Command::Highlight { slug } => {
            edit_highlight(&ctx, slug.as_deref(), init.version, stdin, stdout)
        }
        Command::Check => check(&ctx),
        Command::Render { render } => render_release(&ctx, &render),
        Command::Cut { version, at } => make_release(root, &options, |pipeline| {
            cut::cut(pipeline, version.as_ref(), at.as_deref())
        }),
        Command::Pick {
            version: Some(version),
            commits,
            resume: false,
        } => make_release(root, &options, |pipeline| {
            pick::pick(pipeline, &version, &commits)
        }),
        Command::Pick { .. } => make_release(root, &options, pick::resume),
        Command::Finalize { version } => make_release(root, &options, |pipeline| {
            finalize::finalize(pipeline, &version)
        }),
        Command::Build { version, quick } => {
            let profile = if quick {
                Profile::Quick
            } else {
                Profile::Release
            };
            builds(init)
                .ok_or_else(|| {
                    "This workspace has no user-local storage, where `jp release build` keeps \
                     builds."
                        .to_owned()
                })
                .and_then(|builds| {
                    make_release(root, &options, |pipeline| {
                        build::build(pipeline, &builds, &version, profile, &mut |line| {
                            send_print(stdout, &format!("{line}\n"), "chrome", "plain")
                        })
                    })
                })
        }
        Command::Abandon { version } => make_release(root, &options, |pipeline| {
            abandon::abandon(pipeline, &version)
        }),
        Command::Due => due_release(root, &options, ctx.format),
        Command::Status => make_release(root, &options, |pipeline| {
            status::status(pipeline, now()?, builds(init).as_deref())
        }),
        Command::Tools {
            command: ToolsCommand::Init { path, force },
        } => definitions::init(&resolve_dir(root, &path), force),
    };

    match result {
        Ok(output) => {
            if let Some(note) = &output.note {
                send_print(stdout, note, "chrome", "plain")?;
            }
            if !output.text.is_empty() {
                let format = if output.json { "json" } else { "plain" };
                send_print(stdout, &output.text, "content", format)?;
            }
            send_result(stdout, output.code)
        }
        Err(message) => send_exit(stdout, 1, Some(&message)),
    }
}

/// Create the fragments directory and its `highlights/`, and write the README,
/// each only when it isn't there.
///
/// The README is laid out as the workspace formats the fragments directory.
///
/// `highlights/` starts empty, which git doesn't keep, so a checkout has none
/// until a highlight is written; `jp release highlight` creates it then.
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

/// Write or edit the fragment of type `type_` named `slug`, or after the git
/// branch, in the editor of the host speaking protocol `version`.
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

/// Write or edit the highlight named `slug`, or a new one, in the editor of the
/// host speaking protocol `version`.
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

/// The slug a fragment of `type_` is named by when none is given: the one the
/// current git branch's name gives, read in the workspace at `root`.
///
/// The error says to name the fragment, when there's no branch to name it
/// after.
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

/// Ask the host to collect text, and wait for it.
///
/// The host owns the terminal, since the plugin's stdin carries this protocol,
/// so composition runs there.
fn compose(
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
    request: ComposeRequest,
) -> Result<String, String> {
    send(stdout, &PluginToHost::Compose(request))?;

    match read_message(stdin)? {
        HostToPlugin::Composed(response) => response.text.ok_or_else(|| {
            "Nothing composed; run it again with what it asked for given as flags.".to_owned()
        }),
        HostToPlugin::Error(error) => Err(error.message),
        HostToPlugin::Shutdown => Err("Interrupted.".to_owned()),
        other => Err(format!("expected a composed response, got: {other:?}")),
    }
}

/// The name to credit a highlight to when none is given: JP's `user.name`, then
/// git's, read in the workspace at `root`.
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

/// Check every file in the fragments directory.
///
/// Paths are shown relative to the workspace root.
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

/// Print a release's notes or upgrade guide, or the fragments no release has
/// taken yet, as `render` asks.
///
/// Credits link from the configured `repository`, or else from git's `origin`
/// remote's web address, to the configured forge, or to the one that address is
/// on, and the configured headings name each type's section.
/// An upgrade guide with no sections prints nothing, and says why on stderr.
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
        ctx.options.forge,
        ctx.options.repository.as_deref(),
        &ctx.options.headings,
    )
    .map_err(|error| unreadable(fragments, error))?;
    let text = view.render(&release, &ctx.options.headings)?;

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

/// Say whether the next release of the workspace at `root` is due, as text or,
/// when the host prints JSON, as JSON, and exit 1 when it isn't.
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

/// Where builds of the workspace's releases are kept: `release/` in its
/// user-local storage, when it has some.
fn builds(init: &InitMessage) -> Option<Utf8PathBuf> {
    init.paths
        .user_workspace
        .as_ref()
        .map(|storage| storage.join(build::BUILDS))
}

/// The time now, in seconds since the Unix epoch.
fn now() -> Result<i64, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("The system clock is set before 1970: {error}."))?;

    Ok(i64::try_from(now.as_secs()).unwrap_or(i64::MAX))
}

/// Run `command`, which makes a release in the workspace at `root`.
fn make_release(
    root: &Utf8Path,
    options: &Options,
    command: impl FnOnce(&Pipeline<'_>) -> Result<String, String>,
) -> Result<Output, String> {
    let shell = Shell::new();
    let pipeline = Pipeline::new(&shell, root, options)?;

    command(&pipeline).map(Output::from)
}

/// Refuse a fragments directory that isn't there, saying how to make one.
fn existing(fragments: &Fragments<'_>) -> Result<(), String> {
    if fragments.dir.is_dir() {
        return Ok(());
    }

    Err(format!(
        "There is no fragments directory at {}; create it with `jp release init`.",
        fragments.show(&fragments.dir)
    ))
}

/// Why a release couldn't be read, with the paths it names shown from the
/// workspace root.
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

/// Resolve a directory against the workspace root.
///
/// An absolute path is taken as given; a relative one hangs off the root, so
/// the commands behave the same from any subdirectory.
fn resolve_dir(root: &Utf8Path, dir: &Utf8Path) -> Utf8PathBuf {
    if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        root.join(dir)
    }
}

/// The usage text, rendered from the same clap definition the commands parse.
#[must_use]
pub fn help_text() -> String {
    Args::command().render_help().to_string()
}

fn send_describe(stdout: &mut impl Write) -> Result<(), String> {
    let mut describe = DescribeResponse::from_manifest(
        MANIFEST,
        "release",
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

/// End the session with the code a command chose to report its result by, which
/// `jp` doesn't treat as a failure.
fn send_result(stdout: &mut impl Write, code: u8) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Exit(ExitMessage {
            code,
            reason: None,
            expected: code != 0,
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
            "The host stopped responding. Are `jp` and `jp-release` the same build?".to_owned(),
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
