//! `jp-rfd`: read and write a workspace's RFDs.
//!
//! A command plugin providing `jp rfd` and its subcommands, and the tools the
//! assistant reaches through `source = "plugin.command.rfd.<tool>"`.
//! An `init` carrying a tool call is answered by the `tool` module; any other
//! runs the command its arguments name.
//!
//! Settings come from `plugins.command.rfd.options`, read by the `options`
//! module.
//! The RFD directory is resolved against the workspace root from the host's
//! `init` message, so the commands work from any subdirectory.
//!
//! `board` reads the RFDs, the priority board, and the tickets naming each RFD
//! (`links`) into one view, and `render` prints it.
//! `cmd` holds the commands that change RFDs, which write every metadata change
//! through `document::Document`.
//! A command that asks the user something, or reads an RFD's plan through a
//! model, reaches the host through a `Session`; a tool call has no host to ask,
//! so `promote` offers no tracking ticket there.
//!
//! See: `docs/rfd/001-jp-rfd-process.md`,
//! `docs/rfd/116-shareable-rfd-ticket-and-site-tooling.md`

mod author;
mod board;
mod cmd;
mod definitions;
mod document;
mod id;
mod links;
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
    ComposeMode, ComposeOption, ComposeRequest, DescribeResponse, ExitMessage, HostToPlugin,
    InitMessage, OutputFormat, PluginToHost, PrintMessage, ToolCall, ToolOutcomeMessage,
};
use serde_json::Value;
use ticket::TicketId;

use crate::{
    board::{Entry, Loaded},
    cmd::{
        draft::Draft,
        rank::Place,
        track::{self, Filer, Plan, PlanRequest},
    },
    document::Document,
    id::RfdId,
    links::Links,
    options::{Options, ticket_dir, ticket_vocabulary},
    paths::Paths,
    render::Sections,
};

/// What `jp rfd --version` prints after the command's name: the plugin's
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
    name = "jp rfd",
    version = VERSION,
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
    /// With `jp -F json`, every RFD is printed, tagged with its section and
    /// rank.
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
    },

    /// Show one RFD's metadata and where it stands on the priority board.
    Show {
        /// The RFD: a number (`42`, `042`), a draft id (`D05`), or the path of
        /// its file.
        #[arg(value_parser = RfdId::parse_id_or_path)]
        id: RfdId,

        /// Add the document's sections after its metadata.
        #[arg(long)]
        body: bool,
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
        #[arg(long, value_name = "TICKET", value_parser = TicketId::parse_id_or_path)]
        from_ticket: Option<TicketId>,
    },

    /// Advance an RFD one status: Draft, Discussion, Accepted, Implemented.
    ///
    /// A draft gets the next permanent number, or the one given, and every
    /// reference to its draft id is rewritten.
    /// An RFD reaching Accepted is offered its tracking ticket.
    Promote {
        /// The RFD: a number (`42`, `042`), a draft id (`D05`), or the path of
        /// its file.
        #[arg(value_parser = RfdId::parse_id_or_path)]
        id: RfdId,

        /// The permanent number to publish a draft under, such as `100`, when
        /// the next free one is taken on another branch.
        number: Option<RfdId>,
    },

    /// File or refresh an RFD's tracking ticket, printing its id.
    ///
    /// The ticket holds the RFD's Implementation Plan, read out of the document
    /// by a model.
    /// When the plan can't be read, a marker stands in for it, and the next run
    /// reads it again.
    /// Only an Accepted or Implemented RFD carries a tracking ticket.
    Track {
        /// The RFD: a number (`42`, `042`), or the path of its file.
        #[arg(value_parser = RfdId::parse_id_or_path)]
        id: RfdId,
    },

    /// Mark an RFD as in development, filing its tracking ticket if it has none
    /// and moving the ticket to In Progress.
    Start {
        /// The RFD: a number (`42`, `042`), or the path of its file.
        #[arg(value_parser = RfdId::parse_id_or_path)]
        id: RfdId,
    },

    /// Mark an RFD as no longer in development, moving its tracking ticket back
    /// to Todo.
    Stop {
        /// The RFD: a number (`42`, `042`), or the path of its file.
        #[arg(value_parser = RfdId::parse_id_or_path)]
        id: RfdId,
    },

    /// Record that one RFD builds on another's design.
    Extend {
        /// The RFD that extends the other.
        #[arg(value_parser = RfdId::parse_id_or_path)]
        id: RfdId,

        /// The RFD it builds on.
        #[arg(value_parser = RfdId::parse_id_or_path)]
        target: RfdId,
    },

    /// Record that one RFD can't be accepted or implemented before another.
    Require {
        /// The RFD that depends on the other.
        #[arg(value_parser = RfdId::parse_id_or_path)]
        id: RfdId,

        /// The RFD it depends on.
        #[arg(value_parser = RfdId::parse_id_or_path)]
        target: RfdId,
    },

    /// Record that one RFD replaces another.
    Supersede {
        /// The RFD being replaced, Accepted or Implemented.
        #[arg(value_parser = RfdId::parse_id_or_path)]
        id: RfdId,

        /// The RFD that replaces it.
        #[arg(value_parser = RfdId::parse_id_or_path)]
        by: RfdId,
    },

    /// Set an RFD aside, keeping the document and the reason.
    Abandon {
        /// The RFD: a number (`42`, `042`), a draft id (`D05`), or the path of
        /// its file.
        #[arg(value_parser = RfdId::parse_id_or_path)]
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
        /// The RFD: a number (`42`, `042`), a draft id (`D05`), or the path of
        /// its file.
        #[arg(value_parser = RfdId::parse_id_or_path)]
        id: RfdId,

        /// The new title, which also names the file.
        #[arg(required = true)]
        title: Vec<String>,
    },

    /// Move an RFD on the priority board.
    #[command(group(ArgGroup::new("place").required(true)))]
    Rank {
        /// The RFD to move.
        #[arg(value_parser = RfdId::parse_id_or_path)]
        id: RfdId,

        /// Place it directly above this RFD, in its milestone group.
        #[arg(long, group = "place", value_name = "OTHER", value_parser = RfdId::parse_id_or_path)]
        before: Option<RfdId>,

        /// Place it directly below this RFD, in its milestone group.
        #[arg(long, group = "place", value_name = "OTHER", value_parser = RfdId::parse_id_or_path)]
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

/// What a subcommand produced.
///
/// Notes and warnings stay out of the text so `jp -F json` output survives a
/// pipe into `jq`.
#[derive(Debug, Default)]
struct Output {
    /// What a script reads, on stdout.
    text: String,

    /// Whether `text` is one JSON value, which a JSON output format prints as a
    /// record of its own.
    json: bool,

    /// What the command did, on stderr.
    notes: Vec<String>,

    /// What is left to check, on stderr.
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

impl Output {
    /// Add `other`'s text, notes, and warnings after this one's.
    fn extend(&mut self, other: Self) {
        self.text.push_str(&other.text);
        self.notes.extend(other.notes);
        self.warnings.extend(other.warnings);
    }
}

/// The host a command runs under, reached over the protocol.
///
/// Every request waits for its answer before the next one goes out, so no reply
/// has to be matched to its request by id.
struct Session<'a, R, W> {
    stdin: &'a mut R,
    stdout: &'a mut W,
}

impl<R: BufRead, W: Write> Session<'_, R, W> {
    /// Print `output` now: notes and warnings on stderr, then the text on
    /// stdout.
    ///
    /// On stderr, which `--quiet` silences, rather than in the log, which only
    /// `-v` shows: a warning names something to check now.
    fn emit(&mut self, output: &Output) -> Result<(), String> {
        for line in output.notes.iter().chain(&output.warnings) {
            print_to(self.stdout, &format!("{line}\n"), "chrome")?;
        }
        if output.json {
            print_json(self.stdout, &output.text)?;
        } else if !output.text.is_empty() {
            print(self.stdout, &output.text)?;
        }

        Ok(())
    }

    /// Ask the user to choose one of a request's options.
    ///
    /// `None` when there is nobody at a terminal to ask, or they declined.
    fn choose(&mut self, request: ComposeRequest) -> Result<Option<String>, String> {
        send(self.stdout, &PluginToHost::Compose(request))?;

        match read_message(self.stdin)? {
            HostToPlugin::Composed(response) => Ok(response.text),
            HostToPlugin::Error(error) => Err(error.message),
            HostToPlugin::Shutdown => Err("Interrupted.".to_owned()),
            other => Err(format!("expected a composed response, got: {other:?}")),
        }
    }

    /// Have the host read an RFD's Implementation Plan through a model.
    ///
    /// A query the host refuses or that fails leaves the plan unavailable; an
    /// interruption ends the command.
    fn read_plan(&mut self, request: &PlanRequest<'_>) -> Result<Plan, String> {
        send(
            self.stdout,
            &PluginToHost::Query(track::plan_query(request)),
        )?;

        loop {
            match read_message(self.stdin)? {
                // The conversation the plan is read in exists; the turn follows.
                HostToPlugin::Created(_) => {}
                HostToPlugin::QueryComplete(done) => return Ok(track::plan_from(done.data)),
                HostToPlugin::Error(error) => return Ok(Plan::Unavailable(error.message)),
                HostToPlugin::Shutdown => return Err("Interrupted.".to_owned()),
                other => return Err(format!("expected the query's result, got: {other:?}")),
            }
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

    /// How `jp` was asked to print: `-F json` makes a listing JSON.
    format: OutputFormat,
}

impl Context<'_> {
    /// Read the board, the way every reading command starts.
    fn board(&self) -> Result<Loaded, String> {
        board::load(
            &self.paths.root,
            &self.paths.rfds,
            Some((&self.paths.tickets, self.links())),
        )
    }

    /// The keys tickets name RFDs under.
    fn links(&self) -> &Links {
        &self.options.links
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
                None => handle_command(&init, &mut Session {
                    stdin: &mut stdin,
                    stdout: &mut stdout,
                }),
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

fn handle_command<R: BufRead, W: Write>(
    init: &InitMessage,
    session: &mut Session<'_, R, W>,
) -> Result<(), String> {
    let parsed = Args::try_parse_from(iter::once("jp rfd".to_owned()).chain(init.args.clone()));

    let args = match parsed {
        Ok(args) => args,
        // `--help` and `--version` surface as errors that aren't failures.
        Err(error) if !error.use_stderr() => {
            print(session.stdout, &error.to_string())?;
            return send_exit(session.stdout, 0, None);
        }
        Err(error) => return send_exit(session.stdout, 1, Some(&error.to_string())),
    };

    let options = match Options::read(&init.options) {
        Ok(options) => options,
        Err(message) => return send_exit(session.stdout, 1, Some(&message)),
    };
    let tickets = match ticket_dir(&init.config) {
        Ok(tickets) => tickets,
        Err(message) => return send_exit(session.stdout, 1, Some(&message)),
    };

    let ctx = Context {
        paths: Paths::new(&init.workspace.root, &options.dir, &tickets),
        config: &init.config,
        options: &options,
        format: init.output_format,
    };

    match execute(&ctx, args.command, session) {
        Ok(output) => {
            session.emit(&output)?;
            send_exit(session.stdout, 0, None)
        }
        Err(message) => send_exit(session.stdout, 1, Some(&message)),
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

/// Show one RFD, with its sections when `body` asks for them.
fn show(ctx: &Context<'_>, id: RfdId, body: bool) -> Result<Output, String> {
    let paths = &ctx.paths;
    let Loaded { board, warnings } = ctx.board()?;
    let path = paths.resolve(id)?;
    let file = paths.show(&path);
    let entry = board
        .entry(&file)
        .ok_or_else(|| format!("{file} is not an RFD: its name has no slug."))?;
    let body = if body {
        Some(document::body(&paths::read(&path)?))
    } else {
        None
    };

    let json = ctx.format.is_json();
    let text = if json {
        render::show_json(&board, entry, body.as_deref())?
    } else {
        render::show_text(&board, entry, body.as_deref())
    };

    Ok(Output {
        text,
        json,
        notes: untracked(paths, ctx.links(), entry).into_iter().collect(),
        warnings,
    })
}

/// How to file the tracking ticket of the RFD `entry` describes, when it is far
/// enough along to carry one and has none; `None` otherwise.
///
/// This is what tells the assistant the labels a tracking ticket takes, since
/// it files one with `ticket_create` rather than with a tool of its own.
fn untracked(paths: &Paths, links: &Links, entry: &Entry) -> Option<String> {
    let carries_one = matches!(entry.status.as_deref(), Some("Accepted" | "Implemented"));
    if !carries_one || !paths.tickets.is_dir() || entry.tickets.iter().any(|t| t.tracking) {
        return None;
    }
    let id: RfdId = entry.num.parse().ok()?;

    Some(format!(
        "RFD {id} has no tracking ticket. File one as a ticket labelled {}, or with `jp rfd track \
         {id}`.",
        links.describe_tracking(id)
    ))
}

fn execute<R: BufRead, W: Write>(
    ctx: &Context<'_>,
    command: Command,
    session: &mut Session<'_, R, W>,
) -> Result<Output, String> {
    let paths = &ctx.paths;

    match command {
        Command::List {
            category,
            planned,
            backlog,
            all,
            full,
        } => {
            let Loaded { board, warnings } = ctx.board()?;
            let json = ctx.format.is_json();
            let text = if json {
                render::list_json(&board, category.as_deref())?
            } else {
                let sections = Sections::from_flags(planned, backlog, all);
                render::list_text(&board, category.as_deref(), sections, full)
            };

            Ok(Output {
                text,
                json,
                warnings,
                ..Output::default()
            })
        }

        Command::Show { id, body } => show(ctx, id, body),

        Command::Draft {
            category,
            title,
            from_ticket,
        } => draft(ctx, &category, &title.join(" "), from_ticket),

        Command::Promote { id, number } => promote(ctx, id, number, session),

        Command::Track { id } => {
            let vocabulary = ticket_vocabulary(ctx.config)?;
            let tracked = track::track(
                paths,
                &vocabulary,
                ctx.links(),
                id,
                &user(ctx)?,
                &mut |request| session.read_plan(request),
            )?;

            let mut output = tracked.report;
            output.text = format!("{}\n", tracked.ticket);
            Ok(output)
        }

        Command::Start { id } => {
            let vocabulary = ticket_vocabulary(ctx.config)?;
            track::start(
                paths,
                &vocabulary,
                ctx.links(),
                id,
                &user(ctx)?,
                &mut |request| session.read_plan(request),
            )
        }

        Command::Stop { id } => track::stop(paths, ctx.links(), id),

        Command::Extend { id, target } => cmd::relate::extend(paths, id, target),

        Command::Require { id, target } => cmd::relate::require(paths, id, target),

        Command::Supersede { id, by } => cmd::relate::supersede(paths, id, by),

        Command::Abandon { id, reason } => cmd::abandon::abandon(paths, id, &reason.join(" ")),

        Command::Renumber { id, new } => {
            cmd::renumber::renumber(paths, ctx.links(), &id, new.as_deref(), true)
        }

        Command::Rename { id, title } => cmd::rename::rename(paths, id, &title.join(" "), true),

        Command::Rank {
            id,
            before,
            after,
            backlog: _,
        } => cmd::rank::rank(paths, id, match (before, after) {
            (Some(other), _) => Place::Before(other),
            (_, Some(other)) => Place::After(other),
            (None, None) => Place::Backlog,
        }),

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

/// Write a new draft, from a ticket when one is named, attributed to the user
/// at the terminal.
fn draft(
    ctx: &Context<'_>,
    category: &str,
    title: &str,
    from_ticket: Option<TicketId>,
) -> Result<Output, String> {
    let author = author::resolve(&ctx.paths.root, ctx.config)?;
    let date = today();
    let draft = Draft {
        category,
        title,
        author: &author,
        date: &date,
    };

    match from_ticket {
        Some(ticket) => {
            cmd::draft::from_ticket(&ctx.paths, &ctx.options.categories, ticket, &draft)
        }
        None => cmd::draft::draft(&ctx.paths, &ctx.options.categories, &draft),
    }
}

/// The user at the terminal, filing today.
fn user(ctx: &Context<'_>) -> Result<Filer, String> {
    Ok(Filer {
        author: author::resolve(&ctx.paths.root, ctx.config)?,
        date: today(),
    })
}

/// The value of the tracking offer's choice that files the ticket.
const FILE_TRACKING_TICKET: &str = "file";

/// Advance an RFD one status, offering its tracking ticket when it reaches
/// Accepted.
///
/// The promotion is reported before the question, which is about what it
/// reports.
/// Nothing that goes wrong with the ticket fails the command: the status is
/// already written, and `jp rfd track` files the ticket later.
fn promote<R: BufRead, W: Write>(
    ctx: &Context<'_>,
    id: RfdId,
    number: Option<RfdId>,
    session: &mut Session<'_, R, W>,
) -> Result<Output, String> {
    let file = ctx.paths.resolve(id)?;
    let before = Document::parse(&paths::read(&file)?).header().status();
    let promoted = cmd::promote::promote(&ctx.paths, id, number)?;

    // A project without tickets doesn't track its RFDs.
    if before.as_deref() != Some("Discussion") || !ctx.paths.tickets.is_dir() {
        return Ok(promoted);
    }

    session.emit(&promoted)?;

    let mut output = Output::default();
    let key = &ctx.links().tracking;

    // A board whose labels leave out the link can't take a tracking ticket, so
    // there is nothing to offer. A vocabulary that doesn't load is left to the
    // filing to report.
    if let Ok(vocabulary) = ticket_vocabulary(ctx.config)
        && !vocabulary.is_empty()
        && !vocabulary
            .keys()
            .any(|known| known.eq_ignore_ascii_case(key))
    {
        output.notes.push(format!(
            "Skipped the tracking ticket: the ticket board doesn't define `{key}`. `jp rfd track \
             {id}` shows what to add."
        ));
        return Ok(output);
    }

    let filed = match session.choose(tracking_offer(id)) {
        Ok(Some(answer)) if answer == FILE_TRACKING_TICKET => user(ctx).and_then(|filer| {
            let vocabulary = ticket_vocabulary(ctx.config)?;
            track::track(
                &ctx.paths,
                &vocabulary,
                ctx.links(),
                id,
                &filer,
                &mut |request| session.read_plan(request),
            )
        }),
        // Declined, or nobody at a terminal to ask: acceptance records an
        // agreed direction, not a commitment to start building.
        Ok(_) => {
            output.notes.push(format!(
                "Skipped the tracking ticket. File it later with `jp rfd track {id}`, or as a \
                 ticket labelled {}.",
                ctx.links().describe_tracking(id)
            ));
            return Ok(output);
        }
        Err(error) => Err(error),
    };

    match filed {
        Ok(tracked) => {
            output.text = format!(
                "Review {} before committing it; `jp ticket board` shows the board.\n",
                tracked.ticket
            );
            output.extend(tracked.report);
        }
        Err(error) => output.warnings.push(format!(
            "Could not file the tracking ticket: {error}\nRun `jp rfd track {id}` to retry."
        )),
    }

    Ok(output)
}

/// The question `promote` asks about an RFD it accepted.
fn tracking_offer(id: RfdId) -> ComposeRequest {
    let option = |value: &str, label: &str| ComposeOption {
        value: value.to_owned(),
        label: label.to_owned(),
    };

    ComposeRequest {
        id: None,
        message: format!("File the tracking ticket for RFD {id}?"),
        mode: ComposeMode::Select {
            options: vec![
                option(FILE_TRACKING_TICKET, "Yes"),
                option("skip", "No, file it later"),
            ],
            default: Some(FILE_TRACKING_TICKET.to_owned()),
        },
        help: Some(
            "The ticket holds the RFD's Implementation Plan, read out of it by a model.".to_owned(),
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
