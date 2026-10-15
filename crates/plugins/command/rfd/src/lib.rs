//! `jp-rfd`: read and write a workspace's RFDs.
//!
//! A command plugin providing `jp rfd` and its subcommands, and the tools the
//! assistant reaches through `source = "plugin.command.rfd.<tool>"`.
//! An `init` carrying a tool call is answered by [`tool`]; any other runs the
//! command its arguments name.
//!
//! Settings come from `plugins.command.rfd.options` ([`options`]).
//! The RFD directory is resolved against the workspace root from the host's
//! `init` message, so the commands work from any subdirectory.
//!
//! [`board`] reads the RFDs, the priority board, and the tracking tickets into
//! one view, and [`render`] prints it.
//! [`cmd`] holds the commands that change RFDs, which write every metadata
//! change through [`document::Document`].
//!
//! See: `docs/rfd/001-jp-rfd-process.md`,
//! `docs/rfd/116-shareable-rfd-ticket-and-site-tooling.md`

mod author;
mod board;
mod cmd;
mod definitions;
mod document;
mod id;
mod options;
mod paths;
mod priority;
mod references;
mod render;
mod tool;

use std::{
    io::{BufRead, Write},
    iter,
};

use camino::{Utf8Path, Utf8PathBuf};
use chrono::Local;
use clap::{ArgGroup, CommandFactory, Parser, Subcommand};
use jp_plugin::message::{
    DescribeResponse, ExitMessage, HostToPlugin, InitMessage, PluginToHost, PrintMessage, ToolCall,
    ToolOutcomeMessage,
};
use serde_json::Value;
use ticket::TicketId;

use crate::{
    board::Loaded,
    cmd::{draft::Draft, rank::Place},
    id::RfdId,
    options::{Options, ticket_dir},
    paths::Paths,
    render::Sections,
};

#[derive(Debug, Parser)]
#[command(
    name = "jp rfd",
    about = "Write and track RFDs, design documents with a lifecycle."
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List RFDs in priority order, optionally of one category.
    ///
    /// Only the planned RFDs are listed unless a section is named.
    List {
        /// Only RFDs of this category, such as `design`.
        category: Option<String>,

        /// The planned RFDs, in priority order.
        #[arg(long)]
        planned: bool,

        /// The RFDs on the board below the planned ones.
        #[arg(long)]
        backlog: bool,

        /// Every RFD: planned, backlog, and those the board doesn't place.
        #[arg(long)]
        all: bool,

        /// Add each RFD's summary and the RFDs it depends on.
        #[arg(long)]
        full: bool,

        /// Print every RFD as JSON, tagged with its section and rank.
        #[arg(long)]
        json: bool,
    },

    /// Show one RFD's metadata and where it stands on the priority board.
    Show {
        /// The RFD: a number (`42`, `042`) or a draft id (`D05`).
        id: RfdId,

        /// Print it as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Create a draft from its category's template.
    ///
    /// The draft takes the first free slot in the drafts directory, and its
    /// file is named after the title.
    Draft {
        /// The kind of document: one of
        /// `plugins.command.rfd.options.categories`, `design`, `decision`,
        /// `guide`, or `process` by default.
        category: String,

        /// The title, which also names the file.
        /// With `--from-ticket`, it replaces the ticket's title.
        #[arg(required_unless_present = "from_ticket")]
        title: Vec<String>,

        /// Seed the draft from this ticket's title and description, then record
        /// on the ticket that it moved there and close it.
        #[arg(long, value_name = "TICKET")]
        from_ticket: Option<TicketId>,
    },

    /// Advance an RFD one status: Draft, Discussion, Accepted, Implemented.
    ///
    /// A draft gets the next permanent number, and every reference to its draft
    /// id is rewritten.
    Promote {
        /// The RFD: a number (`42`, `042`) or a draft id (`D05`).
        id: RfdId,
    },

    /// Record that one RFD builds on another's design.
    Extend {
        /// The RFD that extends the other.
        id: RfdId,

        /// The RFD it builds on.
        target: RfdId,
    },

    /// Record that one RFD can't be accepted or implemented before another.
    Require {
        /// The RFD that depends on the other.
        id: RfdId,

        /// The RFD it depends on.
        target: RfdId,
    },

    /// Record that one RFD replaces another.
    Supersede {
        /// The RFD being replaced, Accepted or Implemented.
        id: RfdId,

        /// The RFD that replaces it.
        by: RfdId,
    },

    /// Set an RFD aside, keeping the document and the reason.
    Abandon {
        /// The RFD: a number (`42`, `042`) or a draft id (`D05`).
        id: RfdId,

        /// Why it was set aside.
        #[arg(required = true)]
        reason: Vec<String>,
    },

    /// Move an RFD to another id in its id space.
    ///
    /// A draft moves to another draft slot, and a published RFD to another
    /// permanent number.
    Renumber {
        /// The RFD: an id, or its file name, which picks one of two files
        /// sharing an id after a merge.
        id: String,

        /// The new id.
        /// The next free one when left out; pass it when that one is taken on
        /// another branch.
        new: Option<String>,
    },

    /// Give an RFD a new title, moving its file to match.
    Rename {
        /// The RFD: a number (`42`, `042`) or a draft id (`D05`).
        id: RfdId,

        /// The new title, which also names the file.
        #[arg(required = true)]
        title: Vec<String>,
    },

    /// Move an RFD on the priority board.
    #[command(group(ArgGroup::new("place").required(true)))]
    Rank {
        /// The RFD to move.
        id: RfdId,

        /// Place it directly above this RFD, in its milestone group.
        #[arg(long, group = "place", value_name = "OTHER")]
        before: Option<RfdId>,

        /// Place it directly below this RFD, in its milestone group.
        #[arg(long, group = "place", value_name = "OTHER")]
        after: Option<RfdId>,

        /// Move it to the backlog.
        #[arg(long, group = "place")]
        backlog: bool,
    },

    /// Set up the RFD directory: its drafts directory, a template for each
    /// category, and an empty priority board.
    Init,

    /// Write the tool definitions that offer the RFDs to the assistant.
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
    /// Writes the configured categories into the schemas of the tools that take
    /// one, found by their `source`, as a file of its own that is rewritten
    /// whole.
    Sync {
        /// Directory holding the definitions, relative to the workspace root.
        path: Utf8PathBuf,
    },
}

/// What a subcommand produced: text for the user, and warnings about what is
/// left to check.
///
/// Warnings stay out of the text so `--json` output survives a pipe into `jq`.
#[derive(Debug, Default)]
struct Output {
    text: String,
    warnings: Vec<String>,
}

impl From<String> for Output {
    fn from(text: String) -> Self {
        Self {
            text,
            warnings: vec![],
        }
    }
}

/// What a command runs against.
struct Context<'a> {
    paths: Paths,

    /// The configuration the host resolved.
    config: &'a Value,

    /// The plugin's own settings.
    options: &'a Options,
}

impl Context<'_> {
    /// Read the board, the way every reading command starts.
    fn board(&self) -> Result<Loaded, String> {
        board::load(&self.paths.root, &self.paths.rfds, &self.paths.tickets)
    }
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
                None => handle_command(&init, &mut stdout),
            }
        }
        other => Err(format!("expected init or describe, got: {other:?}")),
    }
}

/// The protocol version this plugin needs from the host.
///
/// 12 is the first to carry tool calls (`tool` on `init`, answered with
/// `tool_outcome`), which is how the `rfd_*` tools reach this plugin.
const REQUIRED_PROTOCOL: u32 = 12;

/// Where this plugin attaches to `jp`, readable from the binary without running
/// it.
///
/// `protocol` has to match `REQUIRED_PROTOCOL`.
pub static MANIFEST: &str = jp_plugin::manifest!(
    protocol: 12,
    description: "Write and track RFDs",
    command: ["rfd"],
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

fn handle_command(init: &InitMessage, stdout: &mut impl Write) -> Result<(), String> {
    let parsed = Args::try_parse_from(iter::once("jp rfd".to_owned()).chain(init.args.clone()));

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
    let tickets = match ticket_dir(&init.config) {
        Ok(tickets) => tickets,
        Err(message) => return send_exit(stdout, 1, Some(&message)),
    };

    let ctx = Context {
        paths: Paths::new(&init.workspace.root, &options.dir, &tickets),
        config: &init.config,
        options: &options,
    };

    match execute(&ctx, args.command) {
        Ok(output) => {
            // On stderr, which `--quiet` silences, rather than in the log,
            // which only `-v` shows: a warning names something to check now.
            for warning in &output.warnings {
                print_to(stdout, &format!("{warning}\n"), "chrome")?;
            }
            if !output.text.is_empty() {
                print(stdout, &output.text)?;
            }
            send_exit(stdout, 0, None)
        }
        Err(message) => send_exit(stdout, 1, Some(&message)),
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

/// Today, as a new draft's `Date` reads.
fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

fn execute(ctx: &Context<'_>, command: Command) -> Result<Output, String> {
    let paths = &ctx.paths;

    match command {
        Command::List {
            category,
            planned,
            backlog,
            all,
            full,
            json,
        } => {
            let Loaded { board, warnings } = ctx.board()?;
            let text = if json {
                render::list_json(&board, category.as_deref())?
            } else {
                let sections = Sections::from_flags(planned, backlog, all);
                render::list_text(&board, category.as_deref(), sections, full)
            };

            Ok(Output { text, warnings })
        }

        Command::Show { id, json } => {
            let Loaded { board, warnings } = ctx.board()?;
            let file = paths.show(&paths.resolve(id)?);
            let entry = board
                .entry(&file)
                .ok_or_else(|| format!("{file} is not an RFD: its name has no slug."))?;

            let text = if json {
                render::show_json(&board, entry)?
            } else {
                render::show_text(&board, entry)
            };

            Ok(Output { text, warnings })
        }

        Command::Draft {
            category,
            title,
            from_ticket,
        } => {
            let author = author::resolve(&paths.root, ctx.config)?;
            let (date, title) = (today(), title.join(" "));
            let draft = Draft {
                category: &category,
                title: &title,
                author: &author,
                date: &date,
            };

            match from_ticket {
                Some(ticket) => {
                    cmd::draft::from_ticket(paths, &ctx.options.categories, ticket, &draft)
                }
                None => cmd::draft::draft(paths, &ctx.options.categories, &draft),
            }
        }

        Command::Promote { id } => cmd::promote::promote(paths, id),

        Command::Extend { id, target } => cmd::relate::extend(paths, id, target),

        Command::Require { id, target } => cmd::relate::require(paths, id, target),

        Command::Supersede { id, by } => cmd::relate::supersede(paths, id, by),

        Command::Abandon { id, reason } => cmd::abandon::abandon(paths, id, &reason.join(" ")),

        Command::Renumber { id, new } => cmd::renumber::renumber(paths, &id, new.as_deref(), true),

        Command::Rename { id, title } => cmd::rename::rename(paths, id, &title.join(" "), true),

        Command::Rank {
            id,
            before,
            after,
            backlog: _,
        } => {
            let place = match (before, after) {
                (Some(other), _) => Place::Before(other),
                (_, Some(other)) => Place::After(other),
                (None, None) => Place::Backlog,
            };
            cmd::rank::rank(paths, id, place)
        }

        Command::Init => cmd::init::init(paths, &ctx.options.categories),

        Command::Tools {
            command: ToolsCommand::Init { path },
        } => definitions::init(&resolve_dir(&paths.root, &path)),

        Command::Tools {
            command: ToolsCommand::Sync { path },
        } => definitions::sync(
            &resolve_dir(&paths.root, &path),
            ctx.config,
            &ctx.options.categories,
        ),
    }
}

/// The usage text, rendered from the same clap definition the commands parse.
#[must_use]
pub fn help_text() -> String {
    Args::command().render_help().to_string()
}

fn send_describe(stdout: &mut impl Write) -> Result<(), String> {
    let mut describe =
        DescribeResponse::from_manifest(MANIFEST, "rfd", env!("CARGO_PKG_VERSION"), help_text())
            .map_err(|e| format!("invalid embedded manifest: {e}"))?;

    describe.author = Some("Jean Mertz <git@jeanmertz.com>".to_owned());
    describe.repository = Some("https://github.com/dcdpr/jp".to_owned());

    send(stdout, &PluginToHost::Describe(describe))
}

fn print(stdout: &mut impl Write, text: &str) -> Result<(), String> {
    print_to(stdout, text, "content")
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
            "The host stopped responding. Are `jp` and `jp-rfd` the same build?".to_owned(),
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
