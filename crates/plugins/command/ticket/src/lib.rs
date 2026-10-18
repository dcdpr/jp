//! `jp-ticket`: read and write a workspace's in-repo tickets.
//!
//! A command plugin providing `jp ticket` and its subcommands, and the tools
//! the assistant reaches through `source = "plugin.command.ticket.<tool>"`.
//! An `init` carrying a tool call is answered by [`tool`]; any other runs the
//! command its arguments name.
//!
//! Settings come from `plugins.command.ticket.options` ([`options`]).
//! The ticket directory is resolved against the workspace root from the host's
//! `init` message, so the commands work from any subdirectory.
//!
//! The format lives in the `ticket` crate; this crate is the protocol and
//! argument layer over it.
//!
//! See: `docs/rfd/072-command-plugin-system.md`,
//! `docs/rfd/100-in-repo-ticket-tracking.md`,
//! `docs/rfd/116-shareable-rfd-ticket-and-site-tooling.md`

mod definitions;
mod options;
mod tool;

use std::{
    fs,
    io::{self, BufRead, Write},
};

use camino::{Utf8Path, Utf8PathBuf};
use chrono::{Local, SecondsFormat, Utc};
use clap::{ArgGroup, CommandFactory, Parser, Subcommand};
use jp_github::models::issues::{Comment as IssueComment, Issue};
use jp_plugin::message::{
    ComposeMode, ComposeOption, ComposeRequest, DescribeResponse, ExitMessage, HostToPlugin,
    InitMessage, OutputFormat, PluginToHost, PrintMessage, ToolCall, ToolOutcomeMessage,
};
use serde::Serialize;
use serde_json::Value;
use ticket::{
    Comment, Kind, Labels, Metadata, NewTicket, Selector, Status, Ticket, TicketId, Vocabulary,
    board::{self, Place},
    import::Import,
    store,
};

use crate::options::{LABELS_KEY, Options, leftover_labels_file};

/// What `jp ticket --version` prints after the command's name: the plugin's
/// version, and the revision of the site files it was built alongside.
///
/// `jp vite serve` reads the revision out of the parentheses, to tell whether
/// the site's loaders read what this plugin writes.
const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (site ",
    env!("JP_SITE_REVISION"),
    ")"
);

#[derive(Debug, Parser)]
#[command(
    name = "jp ticket",
    version = VERSION,
    about = "Track work items as markdown files."
)]
struct Args {
    /// Directory holding the ticket files, relative to the workspace root.
    /// Defaults to `plugins.command.ticket.options.dir`, then `docs/ticket`.
    #[arg(long)]
    dir: Option<Utf8PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// File a ticket, at status Todo.
    Add {
        /// Kind of work: `bug`, `feature`, or `chore`.
        /// Omit it to choose.
        kind: Option<Kind>,

        /// One-line summary of the work.
        /// Omit it to compose the ticket inline.
        title: Option<String>,

        /// Who filed it.
        /// Defaults to your JP or git identity.
        #[arg(long)]
        author: Option<String>,

        /// Description.
        #[arg(long)]
        body: Option<String>,

        /// The RFD this work implements, e.g. `045`.
        #[arg(long)]
        implements: Option<String>,

        /// A `key=value` label from the board's vocabulary.
        /// Repeat for more than one.
        #[arg(long = "label")]
        labels: Vec<String>,
    },

    /// Append a comment to a ticket.
    Comment {
        /// Ticket id: `T-02wt0kx`, `T02wt0kx`, or `02wt0kx`.
        /// Omit it to choose.
        id: Option<TicketId>,

        /// Who is commenting.
        /// Defaults to your JP or git identity.
        #[arg(long)]
        author: Option<String>,

        /// Reply to the comment at this 1-based position.
        #[arg(long)]
        re: Option<usize>,

        /// Comment body.
        #[arg(long)]
        body: Option<String>,
    },

    /// Replace a ticket's labels.
    ///
    /// The whole set is written: pass every label the ticket should end up
    /// with, or none to clear them.
    Label {
        /// Ticket id: `T-02wt0kx`, `T02wt0kx`, or `02wt0kx`.
        /// Omit it to choose.
        id: Option<TicketId>,

        /// A `key=value` label from the board's vocabulary.
        /// Repeat for more than one.
        #[arg(long = "label")]
        labels: Vec<String>,
    },

    /// List the labels the board defines.
    Labels,

    /// Mark a ticket as Done.
    Close {
        /// Ticket id: `T-02wt0kx`, `T02wt0kx`, or `02wt0kx`.
        /// Omit it to choose.
        id: Option<TicketId>,
    },

    /// Move a ticket to another column of the board.
    ///
    /// It joins the column below the tickets ranked there; `rank` places it.
    Status {
        /// Ticket id: `T-02wt0kx`, `T02wt0kx`, or `02wt0kx`.
        id: TicketId,

        /// The column: `todo`, `in progress`, or `done`.
        status: Status,
    },

    /// Give a ticket a new title, moving its file to match.
    Rename {
        /// Ticket id: `T-02wt0kx`, `T02wt0kx`, or `02wt0kx`.
        id: TicketId,

        /// New one-line summary.
        title: String,
    },

    /// Rewrite a ticket's description, kind, or status.
    ///
    /// Metadata and comments the flags don't name are left alone.
    Edit {
        /// Ticket id: `T-02wt0kx`, `T02wt0kx`, or `02wt0kx`.
        /// Omit it to choose.
        id: Option<TicketId>,

        /// New description, replacing the old one.
        #[arg(long)]
        body: Option<String>,

        /// New kind: `bug`, `feature`, or `chore`.
        #[arg(long)]
        kind: Option<Kind>,

        /// New status: `todo`, `in progress`, or `done`.
        #[arg(long)]
        status: Option<Status>,
    },

    /// Record what a ticket is waiting on.
    Block {
        /// Ticket id: `T-02wt0kx`, `T02wt0kx`, or `02wt0kx`.
        id: TicketId,

        /// What it waits on: another ticket's id, or a few words.
        #[arg(required_unless_present = "clear")]
        blocker: Option<String>,

        /// Remove what it waits on instead.
        #[arg(long, conflicts_with = "blocker")]
        clear: bool,
    },

    /// Delete a ticket outright.
    ///
    /// Its id is not retired: a later creation in the same time bucket can draw
    /// it again.
    Delete {
        /// Ticket id: `T-02wt0kx`, `T02wt0kx`, or `02wt0kx`.
        /// Omit it to choose.
        id: Option<TicketId>,
    },

    /// Read one ticket, with its comments numbered for replies.
    ///
    /// With `jp -F json`, the ticket is printed as JSON.
    Show {
        /// Ticket id: `T-02wt0kx`, `T02wt0kx`, or `02wt0kx`.
        /// Omit it to choose.
        id: Option<TicketId>,
    },

    /// Print the board: Todo, In Progress, and Done, each in board order.
    ///
    /// Done shows its top `board.done_limit` tickets; `list` shows them all.
    Board,

    /// Move a ticket next to another in the same column of the board.
    #[command(group(ArgGroup::new("place").required(true)))]
    Rank {
        /// Ticket id: `T-02wt0kx`, `T02wt0kx`, or `02wt0kx`.
        id: TicketId,

        /// Place it directly above this ticket.
        #[arg(long, group = "place")]
        before: Option<TicketId>,

        /// Place it directly below this ticket.
        #[arg(long, group = "place")]
        after: Option<TicketId>,
    },

    /// Record that a ticket became an RFD, and close it.
    Promote {
        /// Ticket id: `T-02wt0kx`, `T02wt0kx`, or `02wt0kx`.
        /// Omit it to choose.
        id: Option<TicketId>,

        /// The RFD the work moved to, e.g. `D07` or `045`.
        #[arg(long)]
        to: String,
    },

    /// Give a ticket a fresh id, after CI reports two files claiming one.
    ///
    /// The losing branch runs this: its commits are still rewritable and every
    /// reference to the id on it is unambiguously its own.
    Refresh {
        /// Path to the ticket file.
        ///
        /// A path, not an id: when two files share an id, the id names both.
        path: Utf8PathBuf,

        /// Revision the branch forked from, for deciding which references it
        /// introduced.
        #[arg(long, default_value = "main")]
        base: String,
    },

    /// Import GitHub issues as tickets, or refresh ones already imported.
    Import {
        /// Issue numbers.
        /// Omit them to pick from the open issues.
        numbers: Vec<u64>,

        /// Repository to read the issues from, as `owner/name`.
        #[arg(long)]
        repo: String,

        /// Kind to file a newly imported issue under.
        #[arg(long, default_value = "bug")]
        kind: Kind,
    },

    /// List tickets, ordered by id.
    ///
    /// With `jp -F json`, the tickets are printed as JSON.
    List {
        /// Only tickets at this status: `todo`, `in progress`, or `done`.
        #[arg(long)]
        status: Option<Status>,

        /// Only tickets of this kind: `bug`, `feature`, or `chore`.
        #[arg(long)]
        kind: Option<Kind>,

        /// Only tickets carrying this label, as `key` or `key=value`.
        /// Repeat to require several.
        #[arg(long = "label")]
        labels: Vec<String>,
    },

    /// Create the ticket directory.
    Init,

    /// Write the tool definitions that offer the tickets to the assistant.
    Tools {
        #[command(subcommand)]
        command: ToolsCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ToolsCommand {
    /// Write a starting set of tool definitions, one file per tool.
    ///
    /// Refuses to overwrite a file that exists.
    /// The workspace configuration has to load the directory for the tools to
    /// appear.
    Init {
        /// Directory to write them into, relative to the workspace root.
        path: Utf8PathBuf,
    },

    /// Regenerate what the tool definitions mirror from configuration.
    ///
    /// Writes the board's labels into the schemas of the tools that take them,
    /// found by their `source`, as a file of its own that is rewritten whole.
    Sync {
        /// Directory holding the definitions, relative to the workspace root.
        path: Utf8PathBuf,
    },
}

/// One ticket in `jp -F json ticket show` output.
#[derive(Serialize)]
struct Detail<'a> {
    /// Carried explicitly: the document has no id of its own, and a caller that
    /// reads one ticket needs it to name the ticket again.
    id: TicketId,
    #[serde(flatten)]
    ticket: &'a Ticket,
    path: &'a str,
}

/// One ticket in `jp -F json ticket list` output.
#[derive(Serialize)]
struct Row<'a> {
    id: TicketId,
    title: &'a str,
    #[serde(flatten)]
    metadata: &'a Metadata,
    comments: usize,
    path: &'a str,
}

/// What a subcommand produced: text for the user, and warnings.
///
/// Warnings stay out of the text so `jp -F json` output survives a pipe into
/// `jq`.
#[derive(Debug, Default)]
struct Output {
    text: String,

    /// Whether `text` is one JSON value, which a JSON output format prints as a
    /// record of its own.
    json: bool,

    warnings: Vec<String>,
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
    /// The ticket directory.
    dir: Utf8PathBuf,

    /// The workspace root, which a relative path on the command line resolves
    /// against.
    root: &'a Utf8Path,

    /// The configuration the host resolved.
    config: &'a Value,

    /// The plugin's own settings.
    options: &'a Options,

    /// How `jp` was asked to print: `-F json` makes `show` and `list` JSON.
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
/// `tool_outcome`), which is how the `ticket_*` tools reach this plugin.
const REQUIRED_PROTOCOL: u32 = 12;

/// Where this plugin attaches to `jp`, readable from the binary without running
/// it.
///
/// `protocol` has to match `REQUIRED_PROTOCOL`.
pub static MANIFEST: &str = jp_plugin::manifest!(
    protocol: 12,
    description: "Track work items as markdown files",
    command: ["ticket"],
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
    let parsed = Args::try_parse_from(
        std::iter::once("jp ticket".to_owned()).chain(with_show_alias(&init.args)),
    );

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
    let dir = resolve_dir(
        &init.workspace.root,
        args.dir.as_deref().unwrap_or(&options.dir),
    );

    let command = match compose_missing(&dir, args.command, stdin, stdout) {
        Ok(command) => command,
        Err(message) => return send_exit(stdout, 1, Some(&message)),
    };

    let ctx = Context {
        dir,
        root: &init.workspace.root,
        config: &init.config,
        options: &options,
        format: init.output_format,
    };

    match execute(&ctx, command) {
        Ok(output) => {
            // On stderr, which `--quiet` silences, rather than in the log,
            // which only `-v` shows: a warning names something to check now.
            for warning in &output.warnings {
                print_to(stdout, &format!("Warning: {warning}\n"), "chrome")?;
            }
            if output.json {
                print_json(stdout, &output.text)?;
            } else if !output.text.is_empty() {
                print(stdout, &output.text)?;
            }
            send_exit(stdout, 0, None)
        }
        Err(message) => send_exit(stdout, 1, Some(&message)),
    }
}

/// How composed text divides into a title and a body.
#[derive(Debug, PartialEq, Eq)]
enum Composition {
    /// Nothing to file.
    Empty,
    /// One line: a title on its own.
    Title(String),
    /// A title, a blank line, and the rest.
    TitleAndBody { title: String, body: String },
    /// Prose that runs from the first line into the second, so all of it is
    /// body and the title has to be asked for separately.
    Body(String),
}

impl Composition {
    /// Read composed text the way a commit message reads: subject, blank line,
    /// then the rest.
    ///
    /// Text that runs straight on from the first line has no subject, so it is
    /// all body.
    /// Trailing blank lines never change the reading.
    fn read(text: &str) -> Self {
        let text = text.trim_end();
        let mut lines = text.lines();

        let Some(first) = lines.next().map(str::trim).filter(|line| !line.is_empty()) else {
            return Self::Empty;
        };

        match lines.next() {
            None => Self::Title(first.to_owned()),
            Some(second) if !second.trim().is_empty() => Self::Body(text.to_owned()),
            Some(_) => {
                let body = lines.collect::<Vec<_>>().join("\n").trim().to_owned();
                if body.is_empty() {
                    Self::Title(first.to_owned())
                } else {
                    Self::TitleAndBody {
                        title: first.to_owned(),
                        body,
                    }
                }
            }
        }
    }
}

/// Title used when the composed text is all body and the user names nothing.
const UNTITLED: &str = "untitled";

/// Ask the user for anything the command line didn't carry.
///
/// Composition runs through the host: the plugin's stdin is this protocol, so
/// the host owns the terminal and the editor the `Ctrl+X` escape opens.
fn compose_missing(
    dir: &Utf8Path,
    command: Command,
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
) -> Result<Command, String> {
    match command {
        Command::Add {
            kind,
            title,
            author,
            body,
            implements,
            labels,
        } if kind.is_none() || title.is_none() => {
            let (kind, title, body) = compose_add(kind, title, body, stdin, stdout)?;

            Ok(Command::Add {
                kind: Some(kind),
                title: Some(title),
                author,
                body,
                implements,
                labels,
            })
        }

        Command::Comment {
            id: None,
            author,
            re,
            body,
        } => compose_missing(
            dir,
            Command::Comment {
                id: Some(pick_ticket(dir, stdin, stdout, "Comment on", false)?),
                author,
                re,
                body,
            },
            stdin,
            stdout,
        ),

        // A comment has no title, so the whole buffer is its body.
        Command::Comment {
            id: Some(id),
            author,
            re,
            body: None,
        } => {
            let body = compose(stdin, stdout, ComposeRequest {
                id: None,
                message: format!("Comment on {id}"),
                mode: ComposeMode::Buffer { initial_text: None },
                help: None,
            })?;

            Ok(Command::Comment {
                id: Some(id),
                author,
                re,
                body: Some(body),
            })
        }

        // Closing offers only what is still open.
        Command::Close { id: None } => Ok(Command::Close {
            id: Some(pick_ticket(dir, stdin, stdout, "Close", true)?),
        }),

        Command::Show { id: None } => Ok(Command::Show {
            id: Some(pick_ticket(dir, stdin, stdout, "Show", false)?),
        }),

        Command::Edit {
            id: None,
            body,
            kind,
            status,
        } => Ok(Command::Edit {
            id: Some(pick_ticket(dir, stdin, stdout, "Edit", false)?),
            body,
            kind,
            status,
        }),

        Command::Label { id: None, labels } => Ok(Command::Label {
            id: Some(pick_ticket(dir, stdin, stdout, "Label", false)?),
            labels,
        }),

        Command::Delete { id: None } => Ok(Command::Delete {
            id: Some(pick_ticket(dir, stdin, stdout, "Delete", false)?),
        }),

        Command::Promote { id: None, to } => Ok(Command::Promote {
            id: Some(pick_ticket(dir, stdin, stdout, "Promote", true)?),
            to,
        }),

        // Importing several at once is the common case after a triage sweep.
        Command::Import {
            numbers,
            repo,
            kind,
        } if numbers.is_empty() => Ok(Command::Import {
            numbers: pick_issues(&repo, stdin, stdout)?,
            repo,
            kind,
        }),

        other => Ok(other),
    }
}

/// Ask which of a repository's open issues to import.
fn pick_issues(
    repo: &str,
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
) -> Result<Vec<u64>, String> {
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| format!("`{repo}` is not an `owner/name` pair."))?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("failed to start the async runtime: {error}"))?;
    let issues = runtime.block_on(fetch_open(owner, name))?;

    let options: Vec<ComposeOption> = issues
        .iter()
        // A pull request is an issue on this endpoint, and isn't importable.
        .filter(|issue| issue.pull_request.is_none())
        .map(|issue| ComposeOption {
            value: issue.number.to_string(),
            label: format!("#{:<5} {}", issue.number, issue.title),
        })
        .collect();

    if options.is_empty() {
        return Err(format!("No open issues in {repo}."));
    }

    let chosen = compose_many(stdin, stdout, ComposeRequest {
        id: None,
        message: format!("Import from {repo}"),
        mode: ComposeMode::MultiSelect { options },
        help: Some("Space to select, Enter to import.".to_owned()),
    })?;

    chosen
        .iter()
        .map(|value| {
            value
                .parse()
                .map_err(|_| format!("`{value}` is not an issue number."))
        })
        .collect()
}

/// Ask which kind of work a ticket describes.
fn pick_kind(stdin: &mut impl BufRead, stdout: &mut impl Write) -> Result<Kind, String> {
    let chosen = compose(stdin, stdout, ComposeRequest {
        id: None,
        message: "Kind of work".to_owned(),
        mode: ComposeMode::Select {
            options: [Kind::Bug, Kind::Feature, Kind::Chore]
                .into_iter()
                .map(|kind| ComposeOption {
                    value: kind.to_string(),
                    label: kind.to_string(),
                })
                .collect(),
            default: None,
        },
        help: None,
    })?;

    chosen
        .parse()
        .map_err(|_| format!("`{chosen}` is not a kind."))
}

/// Ask for a one-line title on its own.
///
/// For when the description is already settled and only the summary is missing.
fn ask_title(stdin: &mut impl BufRead, stdout: &mut impl Write) -> Result<String, String> {
    let title = compose(stdin, stdout, ComposeRequest {
        id: None,
        message: "Title".to_owned(),
        mode: ComposeMode::Line {
            default: Some(UNTITLED.to_owned()),
        },
        help: None,
    })?;

    let title = title.trim();

    Ok(if title.is_empty() { UNTITLED } else { title }.to_owned())
}

/// Compose a ticket's title and description, asking for the title separately
/// when the text has no subject line.
fn compose_ticket(
    kind: Option<Kind>,
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
) -> Result<(String, Option<String>), String> {
    let composed = compose(stdin, stdout, ComposeRequest {
        id: None,
        message: kind.map_or_else(
            || "New ticket".to_owned(),
            |kind| format!("New {kind} ticket"),
        ),
        mode: ComposeMode::Buffer { initial_text: None },
        help: Some("First line is the title, then a blank line, then the description.".to_owned()),
    })?;

    match Composition::read(&composed) {
        Composition::Empty => Err("Nothing to file.".to_owned()),
        Composition::Title(title) => Ok((title, None)),
        Composition::TitleAndBody { title, body } => Ok((title, Some(body))),
        // Prose with no subject line: the whole thing is the description, and
        // the title has to be asked for on its own.
        Composition::Body(body) => Ok((ask_title(stdin, stdout)?, Some(body))),
    }
}

/// Ask for the kind, title, and description a new ticket is missing.
fn compose_add(
    kind: Option<Kind>,
    title: Option<String>,
    body: Option<String>,
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
) -> Result<(Kind, String, Option<String>), String> {
    // Kind first: it frames what you're about to write. The title is read out
    // of the composed text, or asked for last.
    let kind = match kind {
        Some(kind) => kind,
        None => pick_kind(stdin, stdout)?,
    };

    // A description given on the command line is a description, not a draft of
    // the whole ticket. Composing from it would read its one line back as a
    // title and file the ticket with no description at all, quietly turning an
    // explicit `--body` into something else.
    let (title, body) = match (title, body) {
        (Some(title), body) => (title, body),
        (None, Some(body)) => (ask_title(stdin, stdout)?, Some(body)),
        (None, None) => compose_ticket(Some(kind), stdin, stdout)?,
    };

    Ok((kind, title, body))
}

/// Ask which ticket to act on.
///
/// `open_only` drops the closed ones, for the actions that only make sense on
/// live work.
fn pick_ticket(
    dir: &Utf8Path,
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
    verb: &str,
    open_only: bool,
) -> Result<TicketId, String> {
    let entries = store::list(dir).map_err(|error| error.to_string())?;

    // The id travels with the entry, not the document: the filename carries it.
    // So the entry has to be kept alongside its ticket rather than mapped away.
    let options: Vec<ComposeOption> = entries
        .iter()
        .filter_map(|entry| entry.ticket.as_ref().ok().map(|ticket| (entry.id, ticket)))
        .filter(|(_, ticket)| !open_only || ticket.metadata.status != Status::Done)
        .map(|(id, ticket)| ComposeOption {
            value: id.to_string(),
            label: format!("{}  {:<12} {}", id, ticket.metadata.status, ticket.title),
        })
        .collect();

    if options.is_empty() {
        return Err("No tickets to choose from.".to_owned());
    }

    let chosen = compose(stdin, stdout, ComposeRequest {
        id: None,
        message: format!("{verb} which ticket?"),
        mode: ComposeMode::Select {
            options,
            default: None,
        },
        help: None,
    })?;

    chosen
        .parse()
        .map_err(|_| format!("`{chosen}` is not a ticket id."))
}

/// Take a value the composer fills in interactively, or explain its absence.
///
/// Reached only when there was no terminal to ask on, since every one of these
/// is prompted for otherwise.
fn required<T>(value: Option<T>, what: &str) -> Result<T, String> {
    value.ok_or_else(|| format!("No {what} given, and no terminal to ask for one."))
}

/// Ask the host for several values at once, and wait for them.
fn compose_many(
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
    request: ComposeRequest,
) -> Result<Vec<String>, String> {
    send(stdout, &PluginToHost::Compose(request))?;

    match read_message(stdin)? {
        HostToPlugin::Composed(response) if response.values.is_empty() => {
            Err("Nothing selected.".to_owned())
        }
        HostToPlugin::Composed(response) => Ok(response.values),
        HostToPlugin::Error(error) => Err(error.message),
        HostToPlugin::Shutdown => Err("Interrupted.".to_owned()),
        other => Err(format!("expected a composed response, got: {other:?}")),
    }
}

/// Read every page of a repository's open issues.
///
/// All of them, not the first hundred: pull requests come back from this
/// endpoint too and are dropped afterwards, so a page of them would otherwise
/// read as a repository with no open issues at all.
async fn fetch_open(owner: &str, repo: &str) -> Result<Vec<Issue>, String> {
    let mut builder = jp_github::Octocrab::builder();
    if let Some(token) = token() {
        builder = builder.personal_token(token);
    }
    let client = builder
        .build()
        .map_err(|error| format!("failed to create the GitHub client: {error}"))?;

    let issues = client.issues(owner, repo);
    let mut all = vec![];
    for page in 1.. {
        let batch = issues
            .list()
            .page(page)
            .per_page(PER_PAGE)
            .send()
            .await
            .map_err(|error| format!("failed to list issues in {owner}/{repo}: {error}"))?;

        let short = batch.len() < usize::from(PER_PAGE);
        all.extend(batch);
        if short {
            break;
        }
    }

    Ok(all)
}

/// Ask the host to collect text, and wait for it.
fn compose(
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
    request: ComposeRequest,
) -> Result<String, String> {
    send(stdout, &PluginToHost::Compose(request))?;

    match read_message(stdin)? {
        HostToPlugin::Composed(response) => response
            .text
            .ok_or_else(|| "Nothing composed; run it again with the text as arguments.".to_owned()),
        HostToPlugin::Error(error) => Err(error.message),
        HostToPlugin::Shutdown => Err("Interrupted.".to_owned()),
        other => Err(format!("expected a composed response, got: {other:?}")),
    }
}

/// Read `jp ticket T-02wt0kx` as `jp ticket show T-02wt0kx`.
///
/// A bare id is the most common thing to type.
/// An exact subcommand name always wins: `comment` and `promote` are seven
/// characters that fold onto the id alphabet, so both parse as ids and would
/// otherwise be swallowed by the alias.
fn with_show_alias(args: &[String]) -> Vec<String> {
    let Some(first) = args.first() else {
        return args.to_vec();
    };

    let is_subcommand = Args::command()
        .get_subcommands()
        .any(|sub| sub.get_name() == first || sub.get_all_aliases().any(|alias| alias == first));
    if is_subcommand || first.parse::<TicketId>().is_err() {
        return args.to_vec();
    }

    let mut expanded = vec!["show".to_owned()];
    expanded.extend(args.iter().cloned());
    expanded
}

/// Decide who to attribute a write to.
///
/// `--author` wins; otherwise JP's own `user.name`, then git's identity, then
/// the OS username (`$USER`, or `%USERNAME%` on Windows).
/// Erroring beats guessing: a ticket signed by the wrong person is worse than
/// one that refuses to be written.
fn resolve_author(explicit: Option<String>, config: &Value) -> Result<String, String> {
    if let Some(author) = explicit.map(|author| author.trim().to_owned())
        && !author.is_empty()
    {
        return Ok(author);
    }

    let configured = config
        .get("user")
        .and_then(|user| user.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty());
    if let Some(name) = configured {
        return Ok(with_email(name.to_owned()));
    }

    if let Some(name) = git_config("user.name") {
        return Ok(with_email(name));
    }

    if let Ok(user) = std::env::var("USER").or_else(|_| std::env::var("USERNAME"))
        && !user.trim().is_empty()
    {
        return Ok(with_email(user.trim().to_owned()));
    }

    Err(
        "Don't know who you are. Pass --author, or set `user.name` in your JP config or \
         `user.name` in your git config."
            .to_owned(),
    )
}

/// Append git's email to a bare name, so a ticket's author line reads like an
/// RFD's: `John Doe <git@johndoe.com>`.
fn with_email(name: String) -> String {
    if name.contains('<') {
        return name;
    }

    match git_config("user.email") {
        Some(email) => format!("{name} <{email}>"),
        None => name,
    }
}

/// Give a ticket a fresh id and carry its branch's references over.
///
/// The new id lands in the bucket the ticket was created in, so it keeps its
/// place in time rather than jumping to now.
/// A file the branch touched is only rewritten when `base` does not already
/// name the old id: an occurrence that predates the branch may belong to the
/// ticket that kept the id, and nothing here can tell which.
/// Those files are reported instead.
fn refresh(dir: &Utf8Path, path: &Utf8Path, base: &str) -> Result<Output, String> {
    let path = resolve_ticket_path(dir, path)?;
    let bucket = created_bucket(dir, &path)?;
    let changed = branch_files(dir, base)?;

    let done = store::reassign(dir, &path, bucket).map_err(|error| error.to_string())?;
    let mut output = Output::from(format!("{} -> {} at {}\n", done.old, done.new, done.path));

    let old = done.old.to_string();
    let new = done.new.to_string();
    let mut ambiguous = vec![];

    // A ticket names itself nowhere structurally, so anything left inside the
    // renamed file is prose — and prose naming the old id is ambiguous the same
    // way any other file's is: it may mean the ticket that kept the id.
    //
    // The rename put this file here a line ago, so a read failure is not one of
    // the expected cases `rewrite` tolerates.
    match std::fs::read_to_string(&done.path) {
        Ok(source) if source.contains(&old) => ambiguous.push(done.path.clone()),
        Ok(_) => {}
        Err(error) => return Err(format!("{}: {error}", done.path)),
    }

    for file in changed.iter().chain([&dir.join(board::BOARD_FILE)]) {
        if *file == path {
            continue;
        }
        if names_on_base(dir, base, file, &old) {
            ambiguous.push(file.clone());
            continue;
        }
        if rewrite(file, &old, &new)? {
            output.text.push_str(&format!("  rewrote {file}\n"));
        }
    }

    if !ambiguous.is_empty() {
        let names = ambiguous
            .iter()
            .map(|file| file.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        output.warnings.push(format!(
            "{old} already appeared in these files on {base}, so the occurrence may belong to the \
             ticket that kept the id. Left alone for you to sort out: {names}."
        ));
    }

    Ok(output)
}

/// Run git inside `dir`, so the repository found is the one holding the tickets
/// rather than whatever the process was launched from.
fn git(dir: &Utf8Path, args: &[&str]) -> Option<String> {
    std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
}

/// Resolve a user-supplied ticket path to one that exists.
///
/// A relative path is taken against the repository root first — the spelling
/// `just ticket-refresh` documents — then against the ticket directory, so
/// naming a file from inside `docs/ticket/` works too.
/// Everything downstream gets an absolute path: git runs in the ticket
/// directory, where a workspace-relative path would resolve against the wrong
/// place and silently match no commit.
fn resolve_ticket_path(dir: &Utf8Path, path: &Utf8Path) -> Result<Utf8PathBuf, String> {
    let candidates = if path.is_absolute() {
        vec![path.to_path_buf()]
    } else {
        vec![repo_root(dir)?.join(path), dir.join(path)]
    };

    candidates
        .into_iter()
        .find(|candidate| candidate.exists())
        .ok_or_else(|| format!("No ticket file at {path}."))
}

/// The repository root that holds `dir`.
fn repo_root(dir: &Utf8Path) -> Result<Utf8PathBuf, String> {
    git(dir, &["rev-parse", "--show-toplevel"])
        .map(|text| Utf8PathBuf::from(text.trim()))
        .ok_or_else(|| format!("{dir} is not inside a git repository."))
}

/// Whether `base` already carries `token` in `file`.
///
/// A file that named the id before the branch touched it is ambiguous: the
/// occurrence may be the winning ticket's.
/// A file `base` doesn't have at all is the branch's own, so it is not.
fn names_on_base(dir: &Utf8Path, base: &str, file: &Utf8Path, token: &str) -> bool {
    let Ok(root) = repo_root(dir) else {
        return false;
    };
    let Ok(relative) = file.strip_prefix(&root) else {
        return false;
    };

    git(dir, &["show", &format!("{base}:{relative}")])
        .is_some_and(|content| content.contains(token))
}

/// The bucket of the commit that added `path`.
///
/// Falls back to the current bucket for a file git has never seen, which is
/// what an uncommitted ticket looks like.
fn created_bucket(dir: &Utf8Path, path: &Utf8Path) -> Result<u32, String> {
    let added = git(dir, &[
        "log",
        "--diff-filter=A",
        "--format=%at",
        "-1",
        "--",
        path.as_str(),
    ])
    .and_then(|text| text.trim().parse::<u64>().ok());

    match added {
        Some(seconds) => store::bucket_at(seconds).map_err(|error| error.to_string()),
        None => store::current_bucket().map_err(|error| error.to_string()),
    }
}

/// Every file the branch changed against `base`, as absolute paths.
///
/// git reports them relative to the repository root, which is not where this
/// runs, so they are resolved against it before being handed on.
fn branch_files(dir: &Utf8Path, base: &str) -> Result<Vec<Utf8PathBuf>, String> {
    let root = repo_root(dir)?;

    let changed =
        git(dir, &["diff", "--name-only", &format!("{base}...HEAD")]).ok_or_else(|| {
            format!(
                "`git diff {base}...HEAD` failed; pass --base with a revision this branch forked \
                 from."
            )
        })?;

    Ok(changed
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| root.join(line))
        .collect())
}

/// Replace every `old` with `new` in `file`, reporting whether anything moved.
///
/// A file the branch deleted, or one that isn't text, is skipped rather than
/// failing the run: the caller is working through a list it didn't curate.
/// Any other read failure is an error, so an incomplete repair can't report
/// success.
fn rewrite(file: &Utf8Path, old: &str, new: &str) -> Result<bool, String> {
    let source = match std::fs::read_to_string(file) {
        Ok(source) => source,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::InvalidData
            ) =>
        {
            return Ok(false);
        }
        Err(error) => return Err(format!("{file}: {error}")),
    };
    if !source.contains(old) {
        return Ok(false);
    }

    std::fs::write(file, source.replace(old, new)).map_err(|error| error.to_string())?;

    Ok(true)
}

fn git_config(key: &str) -> Option<String> {
    std::process::Command::new("git")
        .args(["config", "--get", key])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
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

fn execute(ctx: &Context<'_>, command: Command) -> Result<Output, String> {
    let dir = ctx.dir.as_path();
    let config = ctx.config;

    match command {
        Command::Add {
            kind,
            title,
            author,
            body,
            implements,
            labels,
        } => add(
            dir,
            required(kind, "kind")?,
            &required(title, "title")?,
            &resolve_author(author, config)?,
            body.as_deref().unwrap_or_default(),
            implements.as_deref(),
            &resolve_labels(ctx.options, &labels)?,
        ),

        Command::Comment {
            id,
            author,
            re,
            body,
        } => {
            let id = required(id, "ticket id")?;
            let author = resolve_author(author, config)?;
            let body = body.unwrap_or_default();
            if body.trim().is_empty() {
                return Err("Refusing to write an empty comment; pass --body.".to_owned());
            }

            let date = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
            let position = store::append_comment(dir, id, &author, &date, re, body.trim())
                .map_err(|error| error.to_string())?;

            Ok(format!("Added {id}#{position} by {author}\n").into())
        }

        Command::Label { id, labels } => {
            relabel(dir, ctx.options, required(id, "ticket id")?, &labels)
        }

        Command::Labels => vocabulary_listing(dir, ctx.options),

        Command::Close { id } => move_ticket(dir, required(id, "ticket id")?, Status::Done),

        Command::Status { id, status } => move_ticket(dir, id, status),

        Command::Rename { id, title } => rename(dir, id, &title),

        Command::Show { id } => show(dir, required(id, "ticket id")?, ctx.format.is_json()),

        Command::Edit {
            id,
            body,
            kind,
            status,
        } => edit(
            dir,
            required(id, "ticket id")?,
            body.as_deref(),
            kind,
            status,
        ),

        Command::Block { id, blocker, clear } => block(dir, id, blocker.as_deref(), clear),

        Command::Board => show_board(dir, ctx.options.board.done_limit),

        Command::Rank { id, before, after } => rank(dir, id, before, after),

        Command::Delete { id } => {
            let id = required(id, "ticket id")?;
            let path = store::delete(dir, id).map_err(|error| error.to_string())?;

            Ok(format!("Deleted {path}\n").into())
        }

        Command::Promote { id, to } => {
            let id = required(id, "ticket id")?;
            let path = store::promote(dir, id, &to).map_err(|error| error.to_string())?;

            Ok(format!("{path}: promoted to {to}, closed as Done\n").into())
        }

        Command::Refresh { path, base } => refresh(dir, &path, &base),

        Command::Import {
            numbers,
            repo,
            kind,
        } => {
            if numbers.is_empty() {
                return Err("No issue numbers given, and no terminal to ask for one.".to_owned());
            }

            let mut output = Output::default();
            for number in numbers {
                output
                    .text
                    .push_str(&import(dir, number, &repo, kind)?.text);
            }

            Ok(output)
        }

        Command::List {
            status,
            kind,
            labels,
        } => list(dir, status, kind, &labels, ctx.format.is_json()),

        Command::Init => init_dir(dir),

        Command::Tools { command } => tools(ctx, command),
    }
}

/// Move a ticket to the column `status` names.
fn move_ticket(dir: &Utf8Path, id: TicketId, status: Status) -> Result<Output, String> {
    let (path, previous) = store::set_status(dir, id, status).map_err(|error| error.to_string())?;

    Ok(if previous == status {
        format!("{path}: already {status}\n")
    } else {
        format!("{path}: {previous} -> {status}\n")
    }
    .into())
}

/// Write the tool definitions, or what they mirror from configuration.
fn tools(ctx: &Context<'_>, command: ToolsCommand) -> Result<Output, String> {
    match command {
        ToolsCommand::Init { path } => definitions::init(&resolve_dir(ctx.root, &path)),
        ToolsCommand::Sync { path } => definitions::sync(
            &resolve_dir(ctx.root, &path),
            ctx.config,
            &ctx.options.vocabulary()?,
        ),
    }
}

/// Give a ticket a new title, moving its file to the slug the title produces.
fn rename(dir: &Utf8Path, id: TicketId, title: &str) -> Result<Output, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("Refusing to give a ticket an empty title.".to_owned());
    }

    let done = store::edit(dir, id, Some(title), None).map_err(|error| error.to_string())?;

    Ok(if done.from == done.to {
        format!("Retitled {}\n", done.to)
    } else {
        format!("Renamed {} -> {}\n", done.from, done.to)
    }
    .into())
}

/// Record what a ticket waits on, or clear it.
fn block(
    dir: &Utf8Path,
    id: TicketId,
    blocker: Option<&str>,
    clear: bool,
) -> Result<Output, String> {
    let blocker = match (clear, blocker) {
        (true, _) => None,
        (false, Some(blocker)) => Some(blocker),
        (false, None) => return Err("Name what the ticket waits on, or pass --clear.".to_owned()),
    };

    let (path, previous) =
        store::set_blocked_by(dir, id, blocker).map_err(|error| error.to_string())?;

    Ok(match (blocker.map(str::trim), previous) {
        (Some(blocker), _) => format!("{path}: blocked by {blocker}\n"),
        (None, Some(previous)) => format!("{path}: no longer blocked by {previous}\n"),
        (None, None) => format!("{path}: was not blocked\n"),
    }
    .into())
}

/// Print the board, one column after another.
///
/// Stacked rather than side by side, which would wrap on a narrow terminal.
/// Done is cut to its top `done_limit` tickets, and its heading says how many
/// there are.
fn show_board(dir: &Utf8Path, done_limit: usize) -> Result<Output, String> {
    let entries = store::list(dir).map_err(|error| error.to_string())?;
    let order = board::read(dir).map_err(|error| error.to_string())?;

    let mut warnings = vec![];
    let mut tickets: Vec<(TicketId, &Ticket)> = vec![];
    for entry in &entries {
        match &entry.ticket {
            Ok(ticket) => tickets.push((entry.id, ticket)),
            Err(error) => warnings.push(format!("{}: {error}", entry.path)),
        }
    }

    let mut columns = vec![];
    for status in [Status::Todo, Status::InProgress, Status::Done] {
        let column: Vec<TicketId> = tickets
            .iter()
            .filter(|(_, ticket)| ticket.metadata.status == status)
            .map(|(id, _)| *id)
            .collect();
        let laid_out = order.arrange(status, &column);

        let total = laid_out.len();
        let shown = if status == Status::Done {
            total.min(done_limit)
        } else {
            total
        };

        let mut text = if shown < total {
            format!("{status} ({total}, showing {shown})\n")
        } else {
            format!("{status} ({total})\n")
        };
        for id in &laid_out[..shown] {
            if let Some((_, ticket)) = tickets.iter().find(|(candidate, _)| candidate == id) {
                text.push_str(&board_row(*id, ticket));
            }
        }
        columns.push(text);
    }

    Ok(Output {
        text: columns.join("\n"),
        warnings,
        ..Output::default()
    })
}

/// One line of a board column: the status is the column's, so it is left out.
fn board_row(id: TicketId, ticket: &Ticket) -> String {
    let id = id.to_string();
    let kind = ticket.metadata.kind.to_string();
    let blocked = ticket
        .metadata
        .blocked_by
        .as_deref()
        .map_or_else(String::new, |by| format!(" (blocked by {by})"));
    let labels = if ticket.metadata.labels.is_empty() {
        String::new()
    } else {
        format!(" [{}]", ticket.metadata.labels)
    };

    format!("{id:<9} {kind:<8} {}{labels}{blocked}\n", ticket.title)
}

/// Move a ticket directly above or below another in its column.
fn rank(
    dir: &Utf8Path,
    id: TicketId,
    before: Option<TicketId>,
    after: Option<TicketId>,
) -> Result<Output, String> {
    let (place, other) = match (before, after) {
        (Some(other), None) => (Place::Before, other),
        (None, Some(other)) => (Place::After, other),
        _ => return Err("Pass one of --before or --after.".to_owned()),
    };

    let status = board::rank(dir, id, place, other).map_err(|error| error.to_string())?;
    let relation = match place {
        Place::Before => "above",
        Place::After => "below",
    };

    Ok(format!("{id} now sits {relation} {other} in {status}\n").into())
}

/// Create the ticket directory.
///
/// One that exists already is left as it is.
/// The board's order file is not created: a board ranks nothing until a ticket
/// is ranked, and needs no file for that.
fn init_dir(dir: &Utf8Path) -> Result<Output, String> {
    if dir.is_dir() {
        return Ok(format!("{dir} already exists.\n").into());
    }

    fs::create_dir_all(dir).map_err(|error| format!("failed to create {dir}: {error}"))?;

    Ok(format!("Created {dir}\n").into())
}

/// Fetch a GitHub issue and write it into a ticket.
///
/// One way only: replies belong on GitHub and arrive on the next import, so
/// nothing is ever written back.
fn import(dir: &Utf8Path, number: u64, repo: &str, kind: Kind) -> Result<Output, String> {
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| format!("`{repo}` is not an `owner/name` pair."))?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("failed to start the async runtime: {error}"))?;
    let (issue, comments) = runtime.block_on(fetch(owner, name, number))?;

    if issue.pull_request.is_some() {
        return Err(format!("{repo}#{number} is a pull request, not an issue."));
    }

    let comments: Vec<Comment> = comments
        .into_iter()
        .map(|comment| Comment {
            from: format!("gh:{}", comment.user.login),
            date: comment
                .created_at
                .to_rfc3339_opts(SecondsFormat::Secs, true),
            re: None,
            body: comment.body.unwrap_or_default(),
        })
        .collect();

    let count = comments.len();
    let imported = store::import(dir, &Import {
        number,
        title: &issue.title,
        description: issue.body.as_deref().unwrap_or_default(),
        comments,
        kind,
        authors: &format!("gh:{}", issue.user.login),
        date: &issue.created_at.format("%Y-%m-%d").to_string(),
    })
    .map_err(|error| error.to_string())?;

    let verb = if imported.created {
        "Imported"
    } else {
        "Refreshed"
    };

    Ok(format!(
        "{verb} {} from {repo}#{number} at {} ({count} comments)\n",
        imported.id, imported.path
    )
    .into())
}

/// Comments per request; the GitHub maximum, so long threads take few round
/// trips.
const PER_PAGE: u8 = 100;

/// Read an issue and every page of its comments.
async fn fetch(owner: &str, repo: &str, number: u64) -> Result<(Issue, Vec<IssueComment>), String> {
    // Anonymous requests work against public repositories, at a much lower rate
    // limit; a token raises it and reaches private ones.
    let mut builder = jp_github::Octocrab::builder();
    if let Some(token) = token() {
        builder = builder.personal_token(token);
    }
    let client = builder
        .build()
        .map_err(|error| format!("failed to create the GitHub client: {error}"))?;

    let issues = client.issues(owner, repo);
    let issue = issues
        .get(number)
        .await
        .map_err(|error| format!("failed to read {owner}/{repo}#{number}: {error}"))?;

    let mut comments = vec![];
    for page in 1.. {
        let batch = issues
            .list_comments(number)
            .page(page)
            .per_page(PER_PAGE)
            .send()
            .await
            .map_err(|error| format!("failed to read comments on #{number}: {error}"))?;

        let short = batch.len() < usize::from(PER_PAGE);
        comments.extend(batch);
        if short {
            break;
        }
    }

    Ok((issue, comments))
}

fn token() -> Option<String> {
    let non_empty = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());

    non_empty("JP_GITHUB_TOKEN").or_else(|| non_empty("GITHUB_TOKEN"))
}

/// File a ticket at `Todo`.
fn add(
    dir: &Utf8Path,
    kind: Kind,
    title: &str,
    author: &str,
    body: &str,
    implements: Option<&str>,
    labels: &Labels,
) -> Result<Output, String> {
    let date = Local::now().format("%Y-%m-%d").to_string();

    let (id, path) = store::create(dir, &NewTicket {
        kind,
        title,
        authors: author,
        date: &date,
        implements,
        labels,
        description: body,
    })
    .map_err(|error| error.to_string())?;

    Ok(format!("Created {path} ({id})\n").into())
}

/// Replace a ticket's labels with the set the caller named.
///
/// Checked against the ticket rather than against the vocabulary alone, so a
/// retired value the ticket already carries can be listed again and kept.
fn relabel(
    dir: &Utf8Path,
    options: &Options,
    id: TicketId,
    requested: &[String],
) -> Result<Output, String> {
    let vocabulary = options.vocabulary()?;
    let (path, applied) =
        store::set_labels(dir, id, &vocabulary, requested).map_err(|error| error.to_string())?;

    Ok(if applied.is_empty() {
        format!("{path}: labels cleared\n")
    } else {
        format!("{path}: {applied}\n")
    }
    .into())
}

/// Check labels for a ticket being filed.
///
/// Both the refusal and the vocabulary's own error carry enough to act on, so
/// they are handed to the user as they are.
/// A ticket filed without labels doesn't read the vocabulary at all, so a board
/// with a broken vocabulary can still file unlabelled tickets.
fn resolve_labels(options: &Options, requested: &[String]) -> Result<Labels, String> {
    if requested.is_empty() {
        return Ok(Labels::default());
    }

    options
        .vocabulary()?
        .resolve(requested)
        .map_err(|error| error.to_string())
}

/// List the labels the board defines, with what each one covers.
fn vocabulary_listing(dir: &Utf8Path, options: &Options) -> Result<Output, String> {
    let vocabulary = options.vocabulary()?;
    let leftover = leftover_labels_file(dir);

    Ok(describe_vocabulary(&vocabulary, leftover.as_deref().map(Utf8Path::as_str)).into())
}

/// Describe the labels a board defines, with what each one covers.
///
/// `leftover` names the file a board defined its labels in before they moved
/// into its options, for a board that defines none and still has it.
fn describe_vocabulary(vocabulary: &Vocabulary, leftover: Option<&str>) -> String {
    if vocabulary.is_empty() {
        let mut text = format!("This board defines no labels. Add them to `{LABELS_KEY}`.\n");
        if let Some(file) = leftover {
            text.push_str(&format!("{file} is no longer read: move its keys there.\n"));
        }
        return text;
    }

    let mut out = String::new();
    for (key, facet) in vocabulary.facets() {
        out.push_str(&format!("{key}  {}\n", facet.description()));
        for value in facet.values() {
            out.push_str(&format!("  {key}={value}\n"));
        }
        for value in facet.retired() {
            out.push_str(&format!("  {key}={value}  (retired)\n"));
        }
        out.push('\n');
    }

    if vocabulary.facets().any(|(_, f)| f.retired().count() > 0) {
        out.push_str("A retired label stays on a ticket that carries it, but can't be added.\n");
    }

    out
}

/// Apply an edit, touching only the parts the caller named.
fn edit(
    dir: &Utf8Path,
    id: TicketId,
    body: Option<&str>,
    kind: Option<Kind>,
    status: Option<Status>,
) -> Result<Output, String> {
    let failed = |error: store::Error| error.to_string();

    let mut path = match body {
        Some(body) => store::edit(dir, id, None, Some(body)).map_err(failed)?.to,
        None => store::locate_ticket(dir, id).map_err(failed)?,
    };

    if let Some(kind) = kind {
        path = store::set_field(dir, id, "Kind", &kind.to_string()).map_err(failed)?;
    }
    if let Some(status) = status {
        path = store::set_field(dir, id, "Status", &status.to_string()).map_err(failed)?;
    }

    Ok(format!("Edited {path}\n").into())
}

/// Read one ticket, as markdown or as JSON for scripting.
fn show(dir: &Utf8Path, id: TicketId, json: bool) -> Result<Output, String> {
    let entry = store::list(dir)
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|entry| {
            entry
                .path
                .file_name()
                .is_some_and(|name| name.starts_with(&id.file_prefix()))
        })
        .ok_or_else(|| format!("No ticket {id}."))?;

    let ticket = entry.ticket.map_err(|error| format!("{id}: {error}"))?;

    if json {
        let json = serde_json::to_string_pretty(&Detail {
            id: entry.id,
            ticket: &ticket,
            path: entry.path.as_str(),
        })
        .map_err(|error| format!("failed to serialize {id}: {error}"))?;

        return Ok(Output {
            text: format!("{json}\n"),
            json: true,
            ..Output::default()
        });
    }

    // The rendered view names the ticket even though the file doesn't: the id
    // is what a reply or a `Blocked by` has to quote.
    let mut out = format!("# {}: {}\n\n", entry.id, ticket.title);
    out.push_str(&format!("- **Path**: {}\n", entry.path));
    out.push_str(&format!("- **Status**: {}\n", ticket.metadata.status));
    out.push_str(&format!("- **Kind**: {}\n", ticket.metadata.kind));
    for token in ticket.metadata.labels.to_tokens() {
        out.push_str(&format!("- **Label**: {token}\n"));
    }
    if !ticket.description.is_empty() {
        out.push_str(&format!("\n{}\n", ticket.description));
    }
    for (index, comment) in ticket.comments.iter().enumerate() {
        out.push_str(&format!(
            "\n## {}#{} \u{2014} {} at {}\n\n{}\n",
            entry.id,
            index + 1,
            comment.from,
            comment.date,
            comment.body
        ));
    }

    Ok(out.into())
}

/// List tickets, filtered by whatever the caller named.
///
/// Labels are matched as written on the ticket rather than through the
/// vocabulary, so a ticket carrying a label the board has since dropped can
/// still be found and fixed.
fn list(
    dir: &Utf8Path,
    status: Option<Status>,
    kind: Option<Kind>,
    labels: &[String],
    json: bool,
) -> Result<Output, String> {
    let entries = store::list(dir).map_err(|error| error.to_string())?;

    let mut warnings = vec![];
    let mut tickets: Vec<(TicketId, &Ticket, &str)> = vec![];
    for entry in &entries {
        match &entry.ticket {
            Ok(ticket) => tickets.push((entry.id, ticket, entry.path.as_str())),
            Err(error) => warnings.push(format!("{}: {error}", entry.path)),
        }
    }
    let selectors = Selector::parse_all(labels).map_err(|error| error.to_string())?;
    tickets.retain(|(_, ticket, _)| {
        status.is_none_or(|status| status == ticket.metadata.status)
            && kind.is_none_or(|kind| kind == ticket.metadata.kind)
            && ticket.metadata.labels.matches(&selectors)
    });

    let text = if json {
        let rows: Vec<Row<'_>> = tickets
            .iter()
            .map(|(id, ticket, path)| Row {
                id: *id,
                title: &ticket.title,
                metadata: &ticket.metadata,
                comments: ticket.comments.len(),
                path,
            })
            .collect();

        let json = serde_json::to_string_pretty(&rows)
            .map_err(|error| format!("failed to serialize tickets: {error}"))?;

        format!("{json}\n")
    } else {
        tickets
            .iter()
            .map(|(id, ticket, _)| row(*id, ticket))
            .collect()
    };

    Ok(Output {
        text,
        json,
        warnings,
    })
}

/// One line of the human-readable listing.
fn row(id: TicketId, ticket: &Ticket) -> String {
    let id = id.to_string();
    let status = ticket.metadata.status.to_string();
    let kind = ticket.metadata.kind.to_string();
    let blocked = ticket
        .metadata
        .blocked_by
        .as_deref()
        .map_or_else(String::new, |by| format!(" (blocked by {by})"));
    let labels = if ticket.metadata.labels.is_empty() {
        String::new()
    } else {
        format!(" [{}]", ticket.metadata.labels)
    };

    format!(
        "{id:<9} {status:<12} {kind:<8} {}{labels}{blocked}\n",
        ticket.title
    )
}

/// The usage text, rendered from the same clap definition the commands parse.
#[must_use]
pub fn help_text() -> String {
    Args::command().render_help().to_string()
}

fn send_describe(stdout: &mut impl Write) -> Result<(), String> {
    let mut describe =
        DescribeResponse::from_manifest(MANIFEST, "ticket", env!("CARGO_PKG_VERSION"), help_text())
            .map_err(|e| format!("invalid embedded manifest: {e}"))?;

    describe.author = Some("Jean Mertz <git@jeanmertz.com>".to_owned());
    describe.repository = Some("https://github.com/dcdpr/jp".to_owned());

    send(stdout, &PluginToHost::Describe(describe))
}

fn print(stdout: &mut impl Write, text: &str) -> Result<(), String> {
    print_to(stdout, text, "content")
}

/// Print `text`, one JSON value, for the host to emit as a record of its own.
fn print_json(stdout: &mut impl Write, text: &str) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Print(PrintMessage {
            text: text.to_owned(),
            channel: "content".to_owned(),
            format: "json".into(),
            language: None,
        }),
    )
}

/// Print `text` on the host's output channel `channel`.
fn print_to(stdout: &mut impl Write, text: &str, channel: &str) -> Result<(), String> {
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

fn read_message(stdin: &mut impl BufRead) -> Result<HostToPlugin, String> {
    let mut line = String::new();
    let read = stdin
        .read_line(&mut line)
        .map_err(|error| format!("failed to read from host: {error}"))?;

    // EOF: the host closed the pipe without answering, which is what an older
    // `jp` does when it gives up on a message it couldn't read.
    if read == 0 {
        return Err(
            "The host stopped responding. Are `jp` and `jp-ticket` the same build?".to_owned(),
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
