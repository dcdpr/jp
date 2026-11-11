//! `jp-wiki`: read and write a workspace's wiki pages.
//!
//! A command plugin providing `jp wiki` and its subcommands.
//! A wiki page is project knowledge that is neither a design proposal nor a
//! unit of work: a reference that describes the system as it is, or a snapshot
//! of dated research.
//! Pages live in `docs/wiki/<id>-<slug>.md`, named by collision-resistant
//! `W-<id>` ids, and carry the metadata block RFDs and tickets carry.
//!
//! Settings come from `plugins.command.wiki.options`, read by the `options`
//! module, and the directory is resolved against the workspace root from the
//! host's `init` message, so the commands work from any subdirectory.
//!
//! `page` reads and writes one page, `store` the directory, and `order` the
//! order pages are read in.
//! `links`, `section`, and `embed` read what a page links to, its sections, and
//! the sections and source lines it embeds; `check` holds pages to the format
//! and their links to what they point at, and `pin` turns links into source
//! into permalinks.
//! `cmd`, `show`, and `edit` are the commands.
//!
//! `tool` answers the `wiki_*` tools with the same commands, held by `access`
//! to the call's access policy, and `definitions` writes the tools'
//! definitions.
//!
//! See: `docs/rfd/127-in-repo-project-wiki.md`

mod access;
mod author;
mod check;
mod cmd;
mod definitions;
mod edit;
mod embed;
mod git;
mod library;
mod links;
mod options;
mod order;
mod page;
mod pin;
mod section;
mod show;
mod store;
mod tool;

use std::{
    io::{BufRead, Write},
    iter,
};

use camino::{Utf8Path, Utf8PathBuf};
use clap::{ArgGroup, CommandFactory, Parser, Subcommand};
use jp_plugin::message::{
    DescribeResponse, ExitMessage, HostToPlugin, InitMessage, OutputFormat, PluginToHost,
    PrintMessage, ToolCall, ToolOutcomeMessage,
};
use serde_json::Value;
use ticket::TicketId;

use crate::{
    access::Access,
    edit::Changes,
    library::Library,
    options::{Options, Paths},
    page::{PageId, Status},
    store::Wiki,
};

/// What `jp wiki --version` prints after the command's name: the plugin's
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
    name = "jp wiki",
    version = VERSION,
    about = "Keep project knowledge as cross-linked wiki pages."
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a page.
    ///
    /// A page starts as a Reference unless `--status` says otherwise; a
    /// Snapshot's source links are pinned as it is created.
    Add {
        /// The title, which also names the file.
        #[arg(required = true)]
        title: Vec<String>,

        /// `reference` or `snapshot`; `reference` when not given.
        #[arg(long)]
        status: Option<Status>,

        /// The one-sentence summary; `jp wiki check` asks for one until it is
        /// given.
        #[arg(long)]
        summary: Option<String>,

        /// The page's text, below its metadata.
        #[arg(long)]
        body: Option<String>,

        /// The page this one sits under: `W-0a1b2c3`; without one, it is a top
        /// page.
        #[arg(long)]
        parent: Option<PageId>,

        /// A `key=value` label; repeat for more than one.
        #[arg(long = "label")]
        labels: Vec<String>,
    },

    /// Show a page's metadata, what it links to and what links to it, and its
    /// sections.
    Show {
        /// The page: `W-0a1b2c3`, `W0a1b2c3`, or `0a1b2c3`.
        id: PageId,

        /// Show the body, with embeds expanded, instead of the sections.
        #[arg(long)]
        body: bool,

        /// Show only this section, by its anchor or its title.
        #[arg(long, conflicts_with = "body")]
        section: Option<String>,

        /// With `--body` or `--section`, show the text as written, embeds
        /// unexpanded: what `jp wiki edit --body` replaces.
        #[arg(long)]
        raw: bool,
    },

    /// List pages in reading order: each top page, then down its children.
    ///
    /// With `jp -F json`, the pages are printed as JSON.
    List {
        /// Only the pages below this one: `W-0a1b2c3`.
        #[arg(long, value_name = "ID")]
        under: Option<PageId>,

        /// Only pages of this status: `reference`, `snapshot`, or `archived`.
        #[arg(long)]
        status: Option<Status>,

        /// Only pages carrying this label, as `key` or `key=value`; repeat to
        /// require several.
        #[arg(long = "label")]
        labels: Vec<String>,

        /// Only pages whose title, summary, or text contains this, ignoring
        /// case.
        #[arg(long)]
        query: Option<String>,

        /// Only the archived pages.
        #[arg(long, conflicts_with = "all")]
        archived: bool,

        /// The archived pages too, after the rest.
        #[arg(long)]
        all: bool,

        /// Add each page's summary.
        #[arg(long)]
        full: bool,
    },

    /// Change a page.
    ///
    /// Without anything to change, opens the page's file in your editor and
    /// checks it when the editor closes.
    /// The page is checked before it is kept, for the problems the edit brings.
    Edit {
        /// The page: `W-0a1b2c3`.
        id: PageId,

        /// A new title; the file moves to match, and links to it follow.
        #[arg(long)]
        title: Option<String>,

        /// A new one-sentence summary.
        #[arg(long)]
        summary: Option<String>,

        /// A new status: `reference`, `snapshot`, or `archived`.
        /// A page becoming a Snapshot is pinned.
        #[arg(long)]
        status: Option<Status>,

        /// A new parent, `W-0a1b2c3`, or `none` to take it away.
        #[arg(long, value_name = "ID|none")]
        parent: Option<String>,

        /// What replaced an archived page, `W-0a1b2c3` or `RFD 042`, or `none`.
        #[arg(long, value_name = "TARGET|none")]
        superseded_by: Option<String>,

        /// A new body; with `--section`, the new text of that section.
        /// A section text opening with a heading replaces the section's own.
        #[arg(long)]
        body: Option<String>,

        /// The section `--body` replaces, by its anchor or its title.
        #[arg(long, requires = "body")]
        section: Option<String>,

        /// A `key=value` label to add; repeat for more than one.
        #[arg(long = "add-label")]
        add_labels: Vec<String>,

        /// A label to remove: `key=value`, or a bare `key` for every label
        /// under it.
        #[arg(long = "remove-label")]
        remove_labels: Vec<String>,
    },

    /// Give a page a new title, moving its file and the links to it.
    Rename {
        /// The page: `W-0a1b2c3`.
        id: PageId,

        /// The new title.
        #[arg(required = true)]
        title: Vec<String>,
    },

    /// Move a page among the pages sharing its parent, or among the top pages
    /// when it has none.
    #[command(group(ArgGroup::new("place").required(true)))]
    Rank {
        /// The page to move.
        id: PageId,

        /// Place it directly above this page.
        #[arg(long, group = "place", value_name = "OTHER")]
        before: Option<PageId>,

        /// Place it directly below this page.
        #[arg(long, group = "place", value_name = "OTHER")]
        after: Option<PageId>,
    },

    /// Retire a page, keeping it so links to it still resolve.
    ///
    /// Refuses a page with active children.
    Archive {
        /// The page: `W-0a1b2c3`.
        id: PageId,

        /// What replaced it: a page, `W-0a1b2c3`, or an RFD, `RFD 042`.
        #[arg(long, value_name = "TARGET")]
        superseded_by: Option<String>,
    },

    /// Turn a page's relative links into source into permalinks, each to the
    /// newest default-branch commit holding the file as the work tree does.
    Pin {
        /// The page: `W-0a1b2c3`.
        id: PageId,
    },

    /// Move a note ticket into the wiki, then close the ticket as Done with
    /// `Promoted to` naming the page.
    FromTicket {
        /// The ticket: `T-02wt0kx`.
        ticket: TicketId,

        /// The page it sits under: `W-0a1b2c3`; without one, it is a top page.
        #[arg(long)]
        parent: Option<PageId>,

        /// `reference` or `snapshot`; `reference` when not given.
        #[arg(long)]
        status: Option<Status>,

        /// The page's one-sentence summary.
        #[arg(long)]
        summary: Option<String>,

        /// A `key=value` label to add to the ticket's; repeat for more than
        /// one.
        #[arg(long = "label")]
        labels: Vec<String>,
    },

    /// Check every page and its links, and exit 1 when one breaks a rule.
    ///
    /// Orphan pages and citations of superseded RFDs are warnings, which
    /// `--strict` makes errors.
    /// With `jp -F json`, the findings are printed as JSON.
    Check {
        /// Fail on warnings too.
        #[arg(long)]
        strict: bool,
    },

    /// Create the wiki directory and the epoch file its ids count time from.
    Init,

    /// Write the tool definitions that offer the wiki to the assistant.
    Tools {
        #[command(subcommand)]
        command: ToolsCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ToolsCommand {
    /// Write a starting set of tool definitions, one file per tool.
    ///
    /// Existing files that match the shipped definitions are left untouched.
    /// Files with different contents are overwritten if they are tracked by git
    /// with no uncommitted changes; otherwise `--force` is required.
    /// The workspace configuration must load the directory for the tools to
    /// take effect.
    Init {
        /// Directory to write them into, relative to the workspace root.
        path: Utf8PathBuf,

        /// Overwrite files that git cannot restore.
        #[arg(long)]
        force: bool,
    },

    /// Regenerate what the tool definitions mirror from configuration.
    ///
    /// Writes the configured labels into the schemas of the tools that take
    /// them, found by their `source`, as a file of its own that is rewritten
    /// whole.
    Sync {
        /// Directory holding the definitions, relative to the workspace root.
        path: Utf8PathBuf,

        /// Overwrite the synced file even if git cannot restore it or another
        /// plugin wrote it.
        #[arg(long)]
        force: bool,
    },
}

impl Command {
    /// The page an `edit` opens in the editor: one that names nothing to
    /// change.
    fn opens_editor(&self) -> Option<PageId> {
        match self {
            Self::Edit {
                id,
                title: None,
                summary: None,
                status: None,
                parent: None,
                superseded_by: None,
                body: None,
                section: None,
                add_labels,
                remove_labels,
            } if add_labels.is_empty() && remove_labels.is_empty() => Some(*id),
            _ => None,
        }
    }
}

/// Whether a `--parent` or `--superseded-by` value takes the field away.
fn is_none(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("none")
}

/// A `--parent` value: a page id, or `none` to take the parent away.
fn parent_arg(value: &str) -> Result<Option<PageId>, String> {
    if is_none(value) {
        return Ok(None);
    }

    value
        .parse()
        .map(Some)
        .map_err(|_| format!("`{value}` is not a page id; give `W-0a1b2c3`, or `none`."))
}

/// What a subcommand produced.
///
/// Notes and warnings stay out of the text so `jp -F json` output survives a
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

    /// The exit code, for a command that reports a result by it: `check` exits
    /// 1 when it finds a problem, after printing what it found.
    code: u8,
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

/// What a command runs against.
pub(crate) struct Context<'a> {
    pub paths: Paths,

    /// The configuration the host resolved.
    pub config: &'a Value,

    /// The plugin's own settings.
    pub options: &'a Options,

    /// How `jp` was asked to print: `-F json` makes `show`, `list`, and `check`
    /// JSON.
    pub format: OutputFormat,

    /// What the command may read and write: everything, for a command the user
    /// runs; what the call's access policy grants, for a tool.
    pub access: Access,
}

impl Context<'_> {
    /// Read every page the command may read, the way every command starts.
    fn wiki(&self) -> Result<Wiki, String> {
        Wiki::load(&self.paths.wiki, &self.access)
    }

    /// Read the RFDs and tickets a page can name, as far as the command may.
    fn library(&self) -> Library {
        Library::load(&self.paths, &self.access)
    }

    /// The web address permalinks name the repository by.
    fn repository(&self) -> Result<Option<String>, String> {
        options::repository(&self.paths.root, self.config)
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
                None => handle_command(&init, &mut stdin, &mut stdout),
            }
        }
        other => Err(format!("expected init or describe, got: {other:?}")),
    }
}

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

/// The protocol version this plugin needs from the host.
///
/// Opening the editor needs 13, which `edit` checks when it is asked to.
const REQUIRED_PROTOCOL: u32 = 12;

/// Where this plugin attaches to `jp`, readable from the binary without running
/// it.
///
/// `protocol` has to match `REQUIRED_PROTOCOL`.
pub static MANIFEST: &str = jp_plugin::manifest!(
    protocol: 12,
    description: "Keep project knowledge as cross-linked wiki pages",
    command: ["wiki"],
);

fn handle_command(
    init: &InitMessage,
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
) -> Result<(), String> {
    let parsed = Args::try_parse_from(iter::once("jp wiki".to_owned()).chain(init.args.clone()));
    let args = match parsed {
        Ok(args) => args,
        // `--help` and `--version` surface as errors that aren't failures.
        Err(error) if !error.use_stderr() => {
            print_to(stdout, &error.to_string(), "content")?;
            return send_exit(stdout, 0, None);
        }
        Err(error) => return send_exit(stdout, 1, Some(&error.to_string())),
    };

    let options = match Options::read(&init.options) {
        Ok(options) => options,
        Err(message) => return send_exit(stdout, 1, Some(&message)),
    };
    let paths = match Paths::new(&init.workspace.root, &init.config, &options) {
        Ok(paths) => paths,
        Err(message) => return send_exit(stdout, 1, Some(&message)),
    };
    let ctx = Context {
        paths,
        config: &init.config,
        options: &options,
        format: init.output_format,
        access: Access::Unrestricted,
    };

    let result = match args.command.opens_editor() {
        // The editor runs on the host, so this one command needs the protocol
        // stream that `execute` doesn't get.
        Some(id) => edit::through_host(init.version, &ctx, id, stdin, stdout),
        None => execute(&ctx, args.command),
    };

    match result {
        Ok(output) => {
            // On stderr, which `--quiet` silences, rather than in the log,
            // which only `-v` shows: a warning names something to check now.
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
            send_result(stdout, output.code)
        }
        Err(message) => send_exit(stdout, 1, Some(&message)),
    }
}

fn execute(ctx: &Context<'_>, command: Command) -> Result<Output, String> {
    match command {
        Command::Add {
            title,
            status,
            summary,
            body,
            parent,
            labels,
        } => cmd::add(ctx, &cmd::Add {
            title: &title.join(" "),
            status,
            summary: summary.as_deref(),
            body: body.as_deref(),
            parent,
            labels: &labels,
            author: &author::resolve(&ctx.paths.root, ctx.config)?,
        }),

        Command::Show {
            id,
            body,
            section,
            raw,
        } => show::show(ctx, id, body, section.as_deref(), raw),

        Command::List {
            under,
            status,
            labels,
            query,
            archived,
            all,
            full,
        } => cmd::list(ctx, &cmd::Filter {
            under,
            status,
            labels: &labels,
            query: query.as_deref(),
            archived,
            all,
            full,
        }),

        Command::Edit {
            id,
            title,
            summary,
            status,
            parent,
            superseded_by,
            body,
            section,
            add_labels,
            remove_labels,
        } => edit::edit(ctx, id, &Changes {
            title,
            summary,
            status,
            parent: parent.as_deref().map(parent_arg).transpose()?,
            superseded_by: superseded_by.map(|target| (!is_none(&target)).then_some(target)),
            body,
            section,
            add_labels,
            remove_labels,
        }),

        Command::Rename { id, title } => edit::edit(ctx, id, &Changes {
            title: Some(title.join(" ")),
            ..Changes::default()
        }),

        Command::Rank { id, before, after } => match (before, after) {
            (Some(other), _) => cmd::rank(ctx, id, true, other),
            (_, Some(other)) => cmd::rank(ctx, id, false, other),
            (None, None) => Err("Pass one of --before or --after.".to_owned()),
        },

        Command::Archive { id, superseded_by } => cmd::archive(ctx, id, superseded_by.as_deref()),

        Command::Pin { id } => cmd::pin_page(ctx, id),

        Command::FromTicket {
            ticket,
            parent,
            status,
            summary,
            labels,
        } => cmd::from_ticket(ctx, &cmd::FromTicket {
            ticket,
            parent,
            status,
            summary: summary.as_deref(),
            labels: &labels,
        }),

        Command::Check { strict } => cmd::check(ctx, strict),

        Command::Init => cmd::init(ctx),

        Command::Tools {
            command: ToolsCommand::Init { path, force },
        } => definitions::init(&resolve_dir(&ctx.paths.root, &path), force),

        Command::Tools {
            command: ToolsCommand::Sync { path, force },
        } => definitions::sync(
            &resolve_dir(&ctx.paths.root, &path),
            ctx.config,
            ctx.options,
            force,
        ),
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
    let mut describe =
        DescribeResponse::from_manifest(MANIFEST, "wiki", env!("CARGO_PKG_VERSION"), help_text())
            .map_err(|e| format!("invalid embedded manifest: {e}"))?;

    describe.author = Some("Jean Mertz <git@jeanmertz.com>".to_owned());
    describe.repository = Some("https://github.com/dcdpr/jp".to_owned());

    send(stdout, &PluginToHost::Describe(describe))
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

/// End the session with a failure.
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

pub(crate) fn read_message(stdin: &mut impl BufRead) -> Result<HostToPlugin, String> {
    let mut line = String::new();
    let read = stdin
        .read_line(&mut line)
        .map_err(|error| format!("failed to read from host: {error}"))?;

    // EOF: the host closed the pipe without answering, which is what an older
    // `jp` does when it gives up on a message it couldn't read.
    if read == 0 {
        return Err(
            "The host stopped responding. Are `jp` and `jp-wiki` the same build?".to_owned(),
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
