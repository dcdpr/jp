//! `jp-release`: record what changes for users as changelog fragments and
//! highlights.
//!
//! A command plugin providing `jp release` and its subcommands, and the tool
//! the assistant reaches through `source = "plugin.command.release.add"`.
//! An `init` carrying a tool call is answered by the `tool` module; any other
//! runs the command its arguments name.
//!
//! The formats, and the check that holds a fragments directory to them, live in
//! the `changelog` crate; this crate is the commands, their configuration, the
//! composer, and the tool over it.
//! `entry` writes a fragment or highlight, `options` reads the settings from
//! `plugins.command.release.options`, and `definitions` writes the tool's
//! definition.
//!
//! See: `docs/rfd/121-changelog-fragments-and-validation.md`

mod definitions;
mod entry;
mod options;
mod tool;

use std::{
    fs::{self, OpenOptions},
    io::{BufRead, Write},
    iter,
    process::Command as Process,
};

use camino::{Utf8Path, Utf8PathBuf};
use changelog::{Type, UnknownType};
use clap::{CommandFactory, Parser, Subcommand};
use jp_plugin::message::{
    ComposeMode, ComposeOption, ComposeRequest, DescribeResponse, ExitMessage, HostToPlugin,
    InitMessage, PluginToHost, PrintMessage, ToolCall, ToolOutcomeMessage,
};
use serde_json::Value;

use crate::{
    entry::{Entry, Fragments, HIGHLIGHTS, Planned},
    options::Options,
};

/// The README `init` writes into the fragments directory, describing both
/// formats.
const README: &str = include_str!("../templates/README.md");

/// The name of the file [`README`] is written to.
const README_FILE: &str = "README.md";

/// What the composer says while a fragment's entry is written.
const FRAGMENT_HELP: &str = "What changed, for the people using the software. A paragraph \
                             starting with `BREAKING CHANGE:` begins its section of the upgrade \
                             guide.";

/// What the composer says while a highlight's prose is written.
const HIGHLIGHT_HELP: &str =
    "What the feature does for the people using the software, in a few paragraphs.";

#[derive(Debug, Parser)]
#[command(
    name = "jp release",
    version,
    about = "Record what changes for users as changelog fragments and highlights."
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

    /// Write a fragment, or with `--highlight`, a highlight.
    ///
    /// Asks for what the flags don't give.
    /// The file is named after the entry's first words, or the highlight's
    /// title, unless `--slug` names it, and is checked against the format
    /// before it is written.
    Add {
        /// Write a highlight, a section of the release notes for a feature
        /// worth a story, rather than a fragment.
        #[arg(long)]
        highlight: bool,

        /// What the change means for users: `added`, `changed`, `deprecated`,
        /// `removed`, `fixed`, `security`, or `performance`.
        #[arg(long = "type", value_name = "TYPE", conflicts_with = "highlight")]
        type_: Option<Type>,

        /// The fragment's entry, or the highlight's prose, as Markdown.
        #[arg(long)]
        body: Option<String>,

        /// The file's name, without `.md`.
        #[arg(long)]
        slug: Option<String>,

        /// The highlight's title, which heads its section of the release notes.
        #[arg(long, requires = "highlight")]
        title: Option<String>,

        /// Someone to credit the highlight's feature to, such as `@JeanMertz`.
        /// Repeat for more than one.
        #[arg(long = "author", value_name = "AUTHOR", requires = "highlight")]
        authors: Vec<String>,
    },

    /// Check every file in the fragments directory, released ones included, and
    /// exit 1 when one breaks a rule.
    Check,

    /// Write the definition of the tool that offers `add` to the assistant.
    Tools {
        #[command(subcommand)]
        command: ToolsCommand,
    },
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

/// What `add` was given on the command line.
#[derive(Debug)]
struct Add {
    highlight: bool,
    type_: Option<Type>,
    body: Option<String>,
    slug: Option<String>,
    title: Option<String>,
    authors: Vec<String>,
}

/// What a subcommand produced.
#[derive(Debug, Default)]
pub(crate) struct Output {
    text: String,

    /// The exit code, for a command that reports a result by it: `check` exits
    /// 1 when it finds a problem, after printing what it found.
    code: u8,
}

impl From<String> for Output {
    fn from(text: String) -> Self {
        Self { text, code: 0 }
    }
}

/// What a command runs against.
struct Context<'a> {
    /// The fragments directory.
    fragments: Fragments<'a>,

    /// The configuration the host resolved, which names who a highlight is
    /// credited to by default.
    config: &'a Value,
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
    description: "Record what changes for users as changelog fragments",
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
    let ctx = Context {
        fragments: Fragments {
            root,
            dir: resolve_dir(root, &options.fragments_dir),
        },
        config: &init.config,
    };

    let result = match args.command {
        Command::Init => init_dir(&ctx),
        Command::Add {
            highlight,
            type_,
            body,
            slug,
            title,
            authors,
        } => add(
            &ctx,
            Add {
                highlight,
                type_,
                body,
                slug,
                title,
                authors,
            },
            stdin,
            stdout,
        ),
        Command::Check => check(&ctx),
        Command::Tools {
            command: ToolsCommand::Init { path, force },
        } => definitions::init(&resolve_dir(root, &path), force),
    };

    match result {
        Ok(output) => {
            if !output.text.is_empty() {
                print(stdout, &output.text)?;
            }
            send_result(stdout, output.code)
        }
        Err(message) => send_exit(stdout, 1, Some(&message)),
    }
}

/// Create the fragments directory and its `highlights/`, and write the README,
/// each only when it isn't there.
///
/// `highlights/` starts empty, which git doesn't keep, so a checkout has none
/// until a highlight is written; `add` creates it then.
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
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&readme)
            .and_then(|mut file| file.write_all(README.as_bytes()))
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

/// Write a fragment or highlight, asking for what `add` doesn't give.
///
/// The directory and a named slug are checked before anything is asked, so a
/// mistake there doesn't cost the user what they typed.
fn add(
    ctx: &Context<'_>,
    add: Add,
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
) -> Result<Output, String> {
    entry::ready(&ctx.fragments, add.highlight, add.slug.as_deref())?;

    let planned = if add.highlight {
        compose_highlight(ctx, add, stdin, stdout)?
    } else {
        compose_fragment(ctx, add, stdin, stdout)?
    };
    planned.write()?;

    Ok(format!("Wrote {}\n", planned.shown).into())
}

/// Plan a fragment, asking for its type and its entry when `add` gives none.
fn compose_fragment(
    ctx: &Context<'_>,
    add: Add,
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
) -> Result<Planned, String> {
    let type_ = match add.type_ {
        Some(type_) => type_,
        None => pick_type(stdin, stdout)?,
    };
    let slug = add.slug.as_deref();
    let plan = |body: &str| {
        entry::plan(
            &ctx.fragments,
            &Entry::Fragment {
                type_,
                body: body.to_owned(),
            },
            slug,
        )
    };

    match add.body {
        Some(body) => plan(&body),
        None => compose_valid(
            stdin,
            stdout,
            &format!("New `{type_}` fragment"),
            FRAGMENT_HELP,
            plan,
        ),
    }
}

/// Plan a highlight, asking for its title, who it credits, and its prose when
/// `add` gives none.
///
/// The title and the credits are held to the format before the prose is asked
/// for, so what the composer reports is about the prose alone.
fn compose_highlight(
    ctx: &Context<'_>,
    add: Add,
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
) -> Result<Planned, String> {
    let title = match add.title {
        Some(title) => title,
        None => compose(stdin, stdout, ComposeRequest {
            id: None,
            message: "Highlight title".to_owned(),
            mode: ComposeMode::Line { default: None },
            help: Some("Heads the highlight's section of the release notes.".to_owned()),
        })?,
    };
    if title.trim().is_empty() {
        return Err("A highlight needs a title.".to_owned());
    }

    let authors = if add.authors.is_empty() {
        ask_authors(ctx, stdin, stdout)?
    } else {
        add.authors
    };
    let authors: Vec<String> = authors
        .iter()
        .map(|author| author.trim().to_owned())
        .filter(|author| !author.is_empty())
        .collect();
    if authors.is_empty() {
        return Err("A highlight credits at least one author.".to_owned());
    }

    let slug = add.slug.as_deref();
    let plan = |body: &str| {
        entry::plan(
            &ctx.fragments,
            &Entry::Highlight {
                title: title.clone(),
                authors: authors.clone(),
                body: body.to_owned(),
            },
            slug,
        )
    };

    match add.body {
        Some(body) => plan(&body),
        None => compose_valid(
            stdin,
            stdout,
            &format!("Highlight: {}", title.trim()),
            HIGHLIGHT_HELP,
            plan,
        ),
    }
}

/// Ask which of the seven types a fragment is, each shown with what it is used
/// for.
fn pick_type(stdin: &mut impl BufRead, stdout: &mut impl Write) -> Result<Type, String> {
    let width = Type::ALL
        .iter()
        .map(|type_| type_.id().len())
        .max()
        .unwrap_or_default();
    let options = Type::ALL
        .iter()
        .map(|type_| ComposeOption {
            value: type_.id().to_owned(),
            label: format!("{:<width$}  {}", type_.id(), type_.description()),
        })
        .collect();

    let chosen = compose(stdin, stdout, ComposeRequest {
        id: None,
        message: "What does the change mean for users?".to_owned(),
        mode: ComposeMode::Select {
            options,
            default: None,
        },
        help: None,
    })?;

    chosen
        .parse()
        .map_err(|unknown: UnknownType| unknown.to_string())
}

/// Ask who to credit a highlight's feature to, offering the user's own name.
fn ask_authors(
    ctx: &Context<'_>,
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
) -> Result<Vec<String>, String> {
    let answer = compose(stdin, stdout, ComposeRequest {
        id: None,
        message: "Credit the feature to".to_owned(),
        mode: ComposeMode::Line {
            default: identity(ctx.fragments.root, ctx.config),
        },
        help: Some("Separate several with commas, such as `@JeanMertz, @jdoe`.".to_owned()),
    })?;

    Ok(answer.split(',').map(str::to_owned).collect())
}

/// Compose text until `plan` accepts it.
///
/// When `plan` refuses what was written, the composer opens again on it, with
/// the reason as its help, so a slip doesn't cost the user their text;
/// cancelling ends it.
fn compose_valid(
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
    message: &str,
    help: &str,
    plan: impl Fn(&str) -> Result<Planned, String>,
) -> Result<Planned, String> {
    let mut initial_text = None;
    let mut help = help.to_owned();

    loop {
        let text = compose(stdin, stdout, ComposeRequest {
            id: None,
            message: message.to_owned(),
            mode: ComposeMode::Buffer { initial_text },
            help: Some(help),
        })?;

        match plan(&text) {
            Ok(planned) => return Ok(planned),
            Err(problem) => {
                help = format!("Not written: {}.", problem.trim_end_matches('.'));
                initial_text = Some(text);
            }
        }
    }
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
    if !fragments.dir.is_dir() {
        return Err(format!(
            "There is no fragments directory at {}; create it with `jp release init`.",
            fragments.show(&fragments.dir)
        ));
    }

    let mut report = changelog::check(&fragments.dir).map_err(|error| error.to_string())?;
    for problem in &mut report.problems {
        problem.path = Utf8PathBuf::from(fragments.show(&problem.path));
    }

    Ok(Output {
        text: format!("{report}\n"),
        code: u8::from(!report.problems.is_empty()),
    })
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
    send(
        stdout,
        &PluginToHost::Print(PrintMessage {
            text: text.to_owned(),
            channel: "content".to_owned(),
            format: "plain".into(),
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
