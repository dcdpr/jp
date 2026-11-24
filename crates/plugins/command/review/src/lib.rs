//! `jp-review`: review workspace files with the assistant using range-anchored
//! comments and suggestions.
//!
//! Provides `jp review` and its subcommands, plus the `review_reply` and
//! `review_comment` tools.
//! A review tracks versions of its files; each comment is anchored to the
//! review, a file, or a line/column range in a specific version, and projects
//! forward across unchanged lines in later versions until edited lines mark it
//! outdated.
//!
//! `jp review submit` sends pending comments and replies to the assistant in a
//! pass and runs a delegated turn; the assistant responds via `review_reply`
//! and `review_comment`.
//!
//! - `text`, `anchor`, `model`, `pass`, and `prompt` are pure domain modules:
//!   line/column indexing and line-diff projection (`text`), anchor types and
//!   `--at` parsing (`anchor`), review state and validation (`model`, `pass`),
//!   and pass prompt rendering (`prompt`).
//! - `store` manages on-disk review directories, locks, and content-addressed
//!   blobs; `snapshot` reads tracked files from the working tree and `base`
//!   command; `paths` resolves user-supplied paths relative to the working
//!   directory and workspace root.
//! - `cmd`, `comments`, and `submit` implement the CLI commands, rendered via
//!   `view` and `render`; `tool` handles `review_reply` and `review_comment`
//!   tool calls, and `definitions` writes `review.toml` and the tool
//!   definitions.
//! - `server`, `api`, and `running` implement `jp review serve`, exposing the
//!   JSON HTTP API, whose types live in the `jp-review-api` crate, and managing
//!   background pass turns; `ui` holds the pages and files the browser UI is
//!   served from, and `browser` opens them.
//! - `options` parses `plugins.command.review.options`.
//!
//! See: `docs/rfd/136-review-engine-for-workspace-files.md`

mod anchor;
mod api;
mod browser;
mod cmd;
mod comments;
mod definitions;
mod id;
mod model;
mod options;
mod pass;
mod paths;
mod prompt;
mod render;
mod running;
mod server;
mod snapshot;
mod store;
mod submit;
#[cfg(test)]
mod test_support;
mod text;
mod tool;
mod ui;
mod view;

use std::{
    io::{BufRead, Write},
    iter,
    net::IpAddr,
    str::FromStr,
};

use camino::{Utf8Path, Utf8PathBuf};
use clap::{ArgGroup, CommandFactory, Parser, Subcommand};
use jp_plugin::{
    message::{HostToPlugin, InitMessage, PluginToHost, ToolCall, ToolOutcomeMessage},
    origin::parse_host,
    server::{Listen, parse_bind},
    stdio::{self, print_json, print_to, send_exit},
};
use serde_json::json;

use crate::{
    browser::{Browser, SystemBrowser},
    cmd::Context,
    id::{CommentRef, Conversation, ReviewId},
    options::{Options, Overrides},
    pass::PassRef,
    submit::Request,
};

pub(crate) const BINARY: &str = "jp-review";

#[derive(Debug, Parser)]
#[command(
    name = "jp review",
    version,
    about = "Review workspace files with the assistant, through comments anchored to exact ranges \
             of them."
)]
struct Args {
    /// Where reviews are stored, relative to the workspace root, instead of
    /// `plugins.command.review.options.dir`.
    ///
    /// A pass hands it to the assistant's turn, whose replies are stored there
    /// too.
    #[arg(long, value_name = "DIR")]
    dir: Option<StoreDir>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(flatten)]
    Review(Command),

    /// Send the comments and replies waiting on the assistant, and run the turn
    /// that answers them.
    ///
    /// What waits on the assistant is every draft comment and draft reply, and
    /// every sent comment the reviewer wrote in last.
    /// The first pass runs in a new conversation unless `--id` names one, and
    /// later passes in the same.
    /// Each reply and comment the assistant writes is printed as it is stored;
    /// Ctrl-C stops the turn, and what it left unanswered goes in the next
    /// pass.
    Submit(SubmitArgs),

    /// Serve the reviews over a JSON HTTP API, for a browser to review files
    /// in.
    ///
    /// A pass submitted over HTTP runs while the server keeps serving, under
    /// the configurations `plugins.command.review.options.cfg` names.
    /// Stopping the server stops the passes it runs, and waits for each to
    /// record how it ended.
    /// The server has no authentication: anyone who can reach its port can read
    /// and change every review, and start passes that edit workspace files.
    Serve(ServeArgs),

    #[command(flatten)]
    Setup(Setup),
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start a review of workspace files.
    ///
    /// What the files hold is recorded as the review's first version, or as its
    /// second when there is a base command (`--base`, or
    /// `plugins.command.review.options.base`): what that command prints for
    /// each file is recorded first, as version 1.
    Start(StartArgs),

    /// List the reviews, oldest first.
    ///
    /// With `jp -F json`, the reviews are printed as JSON.
    Ls,

    /// Show a review and its comments, each placed in one version of its files.
    ///
    /// A comment on lines that changed since it was written is shown where it
    /// was written, marked outdated.
    /// With `jp -F json`, the review is printed as JSON.
    Show {
        /// The review: `R-1h4k2m9`.
        id: ReviewId,

        /// The version to place the comments in; the latest when not given.
        #[arg(long, value_name = "N")]
        version: Option<u32>,
    },

    /// Show what changed in the review's files from one version to another.
    Diff {
        /// The review: `R-1h4k2m9`.
        id: ReviewId,

        /// The version to compare from; the one before `--to` when not given.
        #[arg(long, value_name = "N")]
        from: Option<u32>,

        /// The version to compare to; the latest when not given.
        #[arg(long, value_name = "M")]
        to: Option<u32>,

        /// Show what a pass changed: from the version it was sent against to
        /// the one recorded when it ended, which holds the assistant's edits.
        /// Its number, or `last`.
        #[arg(
            long,
            value_name = "N|last",
            value_parser = PassRef::parse,
            conflicts_with_all = ["from", "to"]
        )]
        pass: Option<PassRef>,
    },

    /// Add a draft comment, or a suggestion, to a review.
    ///
    /// Without `--at`, the comment is on the review as a whole.
    /// Lines and columns in `--at` are read against the files as they are now,
    /// which are recorded as a new version first when they changed since the
    /// latest.
    Comment {
        /// The review: `R-1h4k2m9`.
        id: ReviewId,

        /// What the comment is on: a file, `docs/a.md`; whole lines of it,
        /// `docs/a.md:42` or `docs/a.md:42-45`; or the text from a line and
        /// column up to but not including another, `docs/a.md:42:5-43:18`.
        #[arg(long, value_name = "PATH[:RANGE]")]
        at: Option<String>,

        /// Text to replace the `--at` range with, which makes the comment a
        /// suggestion; empty to suggest deleting the range.
        #[arg(long, value_name = "TEXT")]
        replace: Option<String>,

        /// The comment, in Markdown; a suggestion can leave it out.
        body: Option<String>,
    },

    /// Write the draft reply on an open comment, replacing any it holds.
    Reply {
        /// The comment: `R-1h4k2m9#2`.
        comment: CommentRef,

        /// The reply, in Markdown.
        body: String,
    },

    /// Change a draft comment, or the draft reply on a comment.
    ///
    /// What isn't given is left as it is.
    Edit {
        /// The comment: `R-1h4k2m9#2`.
        comment: CommentRef,

        /// What the comment is on, as `comment --at` takes it.
        #[arg(long, value_name = "PATH[:RANGE]")]
        at: Option<String>,

        /// The text the suggestion replaces its range with.
        #[arg(long, value_name = "TEXT")]
        replace: Option<String>,

        /// The new body, in Markdown.
        body: Option<String>,
    },

    /// Withdraw a draft comment, or the draft reply on a comment.
    ///
    /// A withdrawn comment's number is not used again.
    Withdraw {
        /// The comment: `R-1h4k2m9#2`.
        comment: CommentRef,
    },

    /// Mark comments resolved.
    ///
    /// A draft, or a comment holding a draft reply, can't be resolved; when one
    /// of the comments named can't be, none of them is.
    #[command(group(ArgGroup::new("which").required(true)))]
    Resolve {
        /// The comments: `R-1h4k2m9#2`.
        #[arg(group = "which", value_name = "COMMENT")]
        comments: Vec<CommentRef>,

        /// Resolve every open comment in this review whose last reply is the
        /// assistant accepting it.
        #[arg(long, group = "which", value_name = "ID")]
        accepted: Option<ReviewId>,
    },

    /// Close a review, after which it takes no changes.
    Close {
        /// The review: `R-1h4k2m9`.
        id: ReviewId,
    },

    /// Open the review list, or a review, in the browser, as the running `jp
    /// review serve` serves it.
    Open {
        /// The review: `R-1h4k2m9`; the review list when not given.
        id: Option<ReviewId>,
    },
}

#[derive(Debug, clap::Args)]
struct StartArgs {
    /// The files to review, relative to the current directory or the workspace
    /// root.
    #[arg(required = true, value_name = "FILE")]
    files: Vec<String>,

    /// What to call the review; its files when not given.
    #[arg(long)]
    title: Option<String>,

    /// The command printing what each file is compared with, as a shell splits
    /// it, instead of `plugins.command.review.options.base`: `git show
    /// HEAD~1:./{path}`.
    #[arg(long, value_name = "COMMAND", conflicts_with = "no_base")]
    base: Option<Words>,

    /// Record no base, whatever `plugins.command.review.options.base` says.
    #[arg(long)]
    no_base: bool,

    /// Open the review in the browser, as the running `jp review serve` serves
    /// it.
    #[arg(long)]
    open: bool,
}

impl StartArgs {
    /// Start the review these arguments ask for, and open it in `browser` when
    /// they say to.
    ///
    /// The review is started whether or not it can be opened.
    fn run(self, ctx: &Context<'_>, browser: &dyn Browser) -> Result<Output, String> {
        let base = match (self.base, self.no_base) {
            (_, true) => None,
            (Some(Words(command)), false) => Some(command),
            (None, false) => ctx.options.base.clone(),
        };

        let (id, mut output) =
            cmd::start(ctx, &self.files, self.title.as_deref(), base.as_deref())?;
        if self.open {
            match cmd::open(ctx, Some(id), browser) {
                Ok(url) => output.notes.push(format!("Opened {url}.")),
                Err(error) => output.warnings.push(error),
            }
        }

        Ok(output)
    }
}

/// A command, as a shell splits it into its program and arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Words(Vec<String>);

impl FromStr for Words {
    type Err = String;

    /// Read a command, such as `git show HEAD:./{path}`, the way a shell splits
    /// it.
    fn from_str(text: &str) -> Result<Self, String> {
        let words = shlex::split(text).ok_or("a quote in it isn't closed")?;
        if words
            .first()
            .is_none_or(|program| program.trim().is_empty())
        {
            return Err("it names no program to run, as in `git show HEAD:./{path}`".to_owned());
        }

        Ok(Self(words))
    }
}

/// A directory to store reviews in, which `--dir` names.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StoreDir(Utf8PathBuf);

impl FromStr for StoreDir {
    type Err = String;

    /// Read a directory, refusing text that names none.
    fn from_str(text: &str) -> Result<Self, String> {
        if text.trim().is_empty() {
            return Err("it is empty".to_owned());
        }

        Ok(Self(Utf8PathBuf::from(text)))
    }
}

#[derive(Debug, clap::Args)]
struct SubmitArgs {
    /// The review: `R-1h4k2m9`.
    id: ReviewId,

    /// Send only these comments, by number: `--only 1,2`.
    #[arg(long, value_delimiter = ',', value_name = "N")]
    only: Vec<u32>,

    /// A message to the assistant beside the comments; on its own, it sends a
    /// pass with nothing else waiting.
    #[arg(short = 'm', long = "message", value_name = "BODY")]
    message: Option<String>,

    /// Run the review's passes in this conversation, `jp-c17629384710`, which
    /// already holds the context of the files.
    #[arg(long = "id", value_name = "CONVERSATION")]
    conversation: Option<Conversation>,

    /// A configuration to run the turn under, after the `cfg` option's, as `jp
    /// query --cfg` takes it; repeat for more than one.
    #[arg(long = "cfg", value_name = "CFG")]
    cfg: Vec<String>,
}

impl SubmitArgs {
    /// The pass these arguments ask for.
    fn request(self) -> Request {
        Request::new(
            self.id,
            self.only,
            self.message,
            self.conversation,
            self.cfg,
        )
    }
}

#[derive(Debug, clap::Args)]
struct ServeArgs {
    /// The address to listen on, an IP address or `localhost`;
    /// `plugins.command.review.options.bind`, or `127.0.0.1`, when not given.
    ///
    /// An address other than loopback, such as `0.0.0.0`, hands every review,
    /// and passes that edit workspace files, to whoever can reach it.
    #[arg(long, value_name = "ADDR", value_parser = parse_bind)]
    bind: Option<IpAddr>,

    /// The port to listen on; `plugins.command.review.options.port`, or 3001,
    /// when not given.
    /// `0` lets the system choose one.
    #[arg(long)]
    port: Option<u16>,

    /// A name to answer to on loopback besides addresses and `localhost`, such
    /// as the one a proxy on this machine forwards; repeat for more than one.
    ///
    /// Replaces `plugins.command.review.options.allowed_hosts`.
    /// A name starting with `.`, such as `.ts.net`, stands for every name under
    /// it too.
    #[arg(long = "allowed-host", value_name = "NAME", value_parser = parse_host)]
    allowed_hosts: Vec<String>,

    /// Open the review list in the browser once the server listens.
    #[arg(long)]
    open: bool,

    /// Serve the browser UI from DIR, as `just build-ui review` writes it,
    /// instead of the one built in; for developing the UI.
    ///
    /// DIR is read on every request, and an open page reloads itself when `just
    /// build-ui` writes a new build there.
    /// A relative DIR is relative to the workspace root.
    #[arg(long, value_name = "DIR")]
    ui_dir: Option<Utf8PathBuf>,
}

impl ServeArgs {
    /// Where these flags say to listen, and the names they say to answer to.
    fn listen(self) -> Listen {
        Listen {
            bind: self.bind,
            port: self.port,
            allowed_hosts: (!self.allowed_hosts.is_empty()).then_some(self.allowed_hosts),
        }
    }
}

/// The commands that write files into the workspace, which read neither the
/// options nor the store.
#[derive(Debug, Subcommand)]
enum Setup {
    /// Write the configuration a pass runs under, `review.toml`, into a
    /// directory.
    ///
    /// Add it to `plugins.command.review.options.cfg` for passes to run under
    /// it.
    /// An existing file that differs is overwritten only when git can restore
    /// it; otherwise `--force` is required.
    Init {
        /// Directory to write it into, relative to the workspace root.
        dir: Utf8PathBuf,

        /// Overwrite a file that git cannot restore.
        #[arg(long)]
        force: bool,
    },

    /// Write the definitions of the tools the assistant answers a review with.
    Tools {
        #[command(subcommand)]
        command: ToolsCommand,
    },
}

impl Setup {
    /// Write the files into the workspace at `root`, which a relative directory
    /// is in.
    fn run(self, root: &Utf8Path) -> Result<Output, String> {
        match self {
            Self::Init { dir, force } => definitions::init(&root.join(dir), force),
            Self::Tools {
                command: ToolsCommand::Init { dir, force },
            } => definitions::tools_init(&root.join(dir), force),
        }
    }
}

#[derive(Debug, Subcommand)]
enum ToolsCommand {
    /// Write `review_reply` and `review_comment`, one file per tool.
    ///
    /// Existing files that match the shipped definitions are left untouched.
    /// Files with different contents are overwritten if they are tracked by git
    /// with no uncommitted changes; otherwise `--force` is required.
    /// The workspace configuration must load the directory for the tools to
    /// take effect.
    Init {
        /// Directory to write them into, relative to the workspace root.
        dir: Utf8PathBuf,

        /// Overwrite files that git cannot restore.
        #[arg(long)]
        force: bool,
    },
}

/// What a command produced.
///
/// Notes and warnings stay out of the text, so `jp -F json` output survives a
/// pipe into `jq`.
#[derive(Debug, Default)]
pub(crate) struct Output {
    /// What a script reads, on stdout.
    text: String,

    /// Whether `text` is one JSON value, which a JSON output format prints as a
    /// record of its own.
    json: bool,

    /// What the command did, on stderr.
    notes: Vec<String>,

    /// What is left to check, on stderr.
    warnings: Vec<String>,

    /// Why the command failed after producing the rest, which it exits with.
    failure: Option<String>,
}

impl From<String> for Output {
    fn from(text: String) -> Self {
        Self {
            text,
            ..Self::default()
        }
    }
}

/// The protocol version this plugin needs from the host.
///
/// 15 is the first to say how a delegated turn ended (`outcome` on
/// `query_complete`), which tells a pass the reviewer stopped from one that
/// finished.
const REQUIRED_PROTOCOL: u32 = 15;

/// Where this plugin attaches to `jp`, readable from the binary without running
/// it.
///
/// `protocol` has to match `REQUIRED_PROTOCOL`.
pub static MANIFEST: &str = jp_plugin::manifest!(
    protocol: 15,
    description: "Review workspace files with the assistant",
    command: ["review"],
);

/// Serve one plugin session: read the host's first message and answer it.
///
/// A `describe` is answered and ends the session; an `init` runs the command
/// its arguments name.
/// A command that opens a page does so in the user's default browser.
pub fn run(
    stdin: impl BufRead + Send + 'static,
    stdout: impl Write + Send + 'static,
) -> Result<(), String> {
    run_with(stdin, stdout, &SystemBrowser)
}

/// [`run`], opening pages in `browser`.
fn run_with(
    mut stdin: impl BufRead + Send + 'static,
    mut stdout: impl Write + Send + 'static,
    browser: &dyn Browser,
) -> Result<(), String> {
    match stdio::read(&mut stdin, BINARY)? {
        HostToPlugin::Describe => stdio::describe(
            &mut stdout,
            MANIFEST,
            "review",
            env!("CARGO_PKG_VERSION"),
            help_text(),
        ),
        HostToPlugin::Init(init) => {
            match jp_plugin::ready(REQUIRED_PROTOCOL, init.version) {
                Ok(ready) => stdio::send(&mut stdout, &PluginToHost::Ready(ready))?,
                Err(exit) => return stdio::send(&mut stdout, &PluginToHost::Exit(exit)),
            }

            match &init.tool {
                Some(call) => answer_tool_call(&init, call, &mut stdout),
                None => handle_command(&init, stdin, stdout, browser),
            }
        }
        other => Err(format!("expected init or describe, got: {other:?}")),
    }
}

/// Answer the tool call `init` carried, then exit.
///
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
    stdio::send(
        stdout,
        &PluginToHost::ToolOutcome(ToolOutcomeMessage { outcome }),
    )?;

    send_exit(stdout, 0, None)
}

fn handle_command(
    init: &InitMessage,
    mut stdin: impl BufRead + Send + 'static,
    mut stdout: impl Write + Send + 'static,
    browser: &dyn Browser,
) -> Result<(), String> {
    let parsed = Args::try_parse_from(iter::once("jp review".to_owned()).chain(init.args.clone()));
    let args = match parsed {
        Ok(args) => args,
        // `--help` and `--version` surface as errors that aren't failures.
        Err(error) if !error.use_stderr() => {
            print_to(&mut stdout, &error.to_string(), "content")?;
            return send_exit(&mut stdout, 0, None);
        }
        Err(error) => return send_exit(&mut stdout, 1, Some(&error.to_string())),
    };

    let mut overrides = Overrides {
        dir: args.dir.map(|StoreDir(dir)| dir),
        ..Overrides::default()
    };
    let result = match args.command {
        Commands::Setup(setup) => setup.run(&init.workspace.root),
        Commands::Review(command) => read_options(init, &overrides)
            .and_then(|options| execute(&Context::new(init, &options)?, command, browser)),
        Commands::Submit(submit) => read_options(init, &overrides).and_then(|options| {
            let ctx = Context::new(init, &options)?;
            submit::submit(&ctx, &submit.request(), &mut stdin, &mut stdout)
        }),
        Commands::Serve(serve) => {
            let open = serve.open.then_some(browser);
            overrides.ui = serve
                .ui_dir
                .as_deref()
                .map(|dir| init.workspace.root.join(dir));
            overrides.listen = serve.listen();
            return server::run(init, &overrides, open, stdin, stdout);
        }
    };

    match result {
        Ok(output) => emit(&mut stdout, &output),
        Err(message) => send_exit(&mut stdout, 1, Some(&message)),
    }
}

/// The options the host passed in `init`, with what the command line sets in
/// their place.
fn read_options(init: &InitMessage, overrides: &Overrides) -> Result<Options, String> {
    Options::read(&init.options).map(|options| options.with(overrides))
}

fn execute(ctx: &Context<'_>, command: Command, browser: &dyn Browser) -> Result<Output, String> {
    match command {
        Command::Start(start) => start.run(ctx, browser),
        Command::Ls => cmd::list(ctx),
        Command::Show { id, version } => cmd::show(ctx, id, version),
        Command::Diff { id, from, to, pass } => match pass {
            Some(pass) => cmd::diff_pass(ctx, id, pass),
            None => cmd::diff(ctx, id, from, to),
        },
        Command::Comment {
            id,
            at,
            replace,
            body,
        } => comments::comment(ctx, id, at.as_deref(), replace, body.as_deref()),
        Command::Reply { comment, body } => comments::reply(ctx, comment, &body),
        Command::Edit {
            comment,
            at,
            replace,
            body,
        } => comments::edit(ctx, comment, at.as_deref(), replace, body),
        Command::Withdraw { comment } => comments::withdraw(ctx, comment),
        Command::Resolve { comments, accepted } => comments::resolve(ctx, &comments, accepted),
        Command::Close { id } => cmd::close(ctx, id),
        Command::Open { id } => {
            let url = cmd::open(ctx, id, browser)?;
            if ctx.format.is_json() {
                return cmd::json(&json!({ "url": url }));
            }

            Ok(format!("{url}\n").into())
        }
    }
}

/// Print what a command produced, and end the session.
fn emit(stdout: &mut impl Write, output: &Output) -> Result<(), String> {
    // On stderr, which `--quiet` silences, rather than in the log, which only
    // `-v` shows: a warning names something to check now.
    for note in &output.notes {
        print_to(stdout, &format!("{note}\n"), "chrome")?;
    }
    for warning in &output.warnings {
        print_to(stdout, &format!("Warning: {warning}\n"), "chrome")?;
    }
    if output.json {
        print_json(stdout, &output.text)?;
    } else if !output.text.is_empty() {
        print_to(stdout, &output.text, "content")?;
    }

    match &output.failure {
        Some(failure) => send_exit(stdout, 1, Some(failure)),
        None => send_exit(stdout, 0, None),
    }
}

/// The usage text, rendered from the same clap definition the commands parse.
#[must_use]
pub fn help_text() -> String {
    Args::command().render_help().to_string()
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
