//! `jp-ticket`: read and write the in-repo tickets under `docs/ticket/`.
//!
//! A command plugin providing `jp ticket create`, `comment`, `close`, and
//! `list`.
//! The ticket directory is resolved against the workspace root from the host's
//! `init` message, so the commands work from any subdirectory.
//!
//! The format lives in the `ticket` crate; this binary is the protocol and
//! argument layer over it.
//!
//! See: `docs/rfd/072-command-plugin-system.md`,
//! `docs/rfd/100-in-repo-ticket-tracking.md`

use std::io::{self, BufRead, BufReader, IsTerminal as _, Write};

use camino::{Utf8Path, Utf8PathBuf};
use chrono::{Local, SecondsFormat, Utc};
use clap::{CommandFactory, Parser, Subcommand};
use jp_github::models::issues::{Comment as IssueComment, Issue};
use jp_plugin::message::{
    ComposeMode, ComposeOption, ComposeRequest, DescribeResponse, ExitMessage, HostToPlugin,
    InitMessage, LogMessage, PluginToHost, PrintMessage,
};
use serde::Serialize;
use serde_json::Value;
use ticket::{Comment, Kind, Metadata, Status, Ticket, TicketId, import::Import, render, store};

#[derive(Debug, Parser)]
#[command(name = "jp ticket", about = "Track work items as markdown files.")]
struct Args {
    /// Directory holding the ticket files, relative to the workspace root.
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

    /// Mark a ticket as Done.
    Close {
        /// Ticket id: `T-02wt0kx`, `T02wt0kx`, or `02wt0kx`.
        /// Omit it to choose.
        id: Option<TicketId>,
    },

    /// Rewrite a ticket's title, description, kind, or status.
    ///
    /// Metadata and comments the flags don't name are left alone.
    Edit {
        /// Ticket id: `T-02wt0kx`, `T02wt0kx`, or `02wt0kx`.
        /// Omit it to choose.
        id: Option<TicketId>,

        /// New one-line summary.
        #[arg(long)]
        title: Option<String>,

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
    Show {
        /// Ticket id: `T-02wt0kx`, `T02wt0kx`, or `02wt0kx`.
        /// Omit it to choose.
        id: Option<TicketId>,

        /// Print JSON instead of markdown.
        #[arg(long)]
        json: bool,
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

    /// Convert tickets left in the pre-RFD-102 `NNNN-slug.md` format.
    ///
    /// Transitional: run it on a branch cut before the id change, after
    /// rebasing.
    /// Delete this command once no such branch is left.
    Migrate,

    /// Import GitHub issues as tickets, or refresh ones already imported.
    Import {
        /// Issue numbers.
        /// Omit them to pick from the open issues.
        numbers: Vec<u64>,

        /// Repository to read the issue from, as `owner/name`.
        #[arg(long, default_value = "dcdpr/jp")]
        repo: String,

        /// Kind to file a newly imported issue under.
        #[arg(long, default_value = "bug")]
        kind: Kind,
    },

    /// List tickets, ordered by id.
    List {
        /// Only tickets at this status: `todo`, `in progress`, or `done`.
        #[arg(long)]
        status: Option<Status>,

        /// Only tickets of this kind: `bug`, `feature`, or `chore`.
        #[arg(long)]
        kind: Option<Kind>,

        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
}

/// One ticket in `show --json` output.
#[derive(Serialize)]
struct Detail<'a> {
    /// Carried explicitly: the document has no id, and `just ticket-promote`
    /// reads this field.
    id: TicketId,
    #[serde(flatten)]
    ticket: &'a Ticket,
    path: &'a str,
}

/// One ticket in `list --json` output.
#[derive(Serialize)]
struct Row<'a> {
    id: TicketId,
    title: &'a str,
    #[serde(flatten)]
    metadata: &'a Metadata,
    comments: usize,
    path: &'a str,
}

/// What a subcommand produced: text for the user, warnings for the log.
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

fn main() {
    // A human running the binary directly gets the help text rather than a
    // hung read on a protocol that is never going to speak.
    if io::stdin().is_terminal() {
        let mut err = io::stderr().lock();
        drop(writeln!(err, "{}", help_text()));
        drop(writeln!(err));
        drop(writeln!(
            err,
            "Note: this binary is a JP plugin. Run it via `jp ticket`."
        ));
        std::process::exit(0);
    }

    let code = match run(BufReader::new(io::stdin()), io::stdout()) {
        Ok(()) => 0,
        Err(error) => {
            let mut err = io::stderr().lock();
            drop(writeln!(err, "Fatal: {error}"));
            1
        }
    };

    std::process::exit(code);
}

fn run(mut stdin: impl BufRead, mut stdout: impl Write) -> Result<(), String> {
    match read_message(&mut stdin)? {
        HostToPlugin::Describe => send_describe(&mut stdout),
        HostToPlugin::Init(init) => {
            match jp_plugin::ready(REQUIRED_PROTOCOL, init.version) {
                Ok(ready) => send(&mut stdout, &PluginToHost::Ready(ready))?,
                Err(exit) => return send(&mut stdout, &PluginToHost::Exit(exit)),
            }
            handle_command(&init, &mut stdin, &mut stdout)
        }
        other => Err(format!("expected init or describe, got: {other:?}")),
    }
}

/// The protocol version this plugin needs from the host.
///
/// Composition (`compose` / `composed`) arrived in 2.
/// Running against an older `jp` means every interactive path sends a message
/// the host cannot read, so the handshake refuses it up front rather than
/// discovering it mid-prompt.
const REQUIRED_PROTOCOL: u32 = 2;

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

    let dir = resolve_dir(&init.workspace.root, args.dir.as_deref());

    let command = match compose_missing(&dir, args.command, stdin, stdout) {
        Ok(command) => command,
        Err(message) => return send_exit(stdout, 1, Some(&message)),
    };

    match execute(&dir, command, &init.config) {
        Ok(output) => {
            for warning in &output.warnings {
                send(
                    stdout,
                    &PluginToHost::Log(LogMessage {
                        level: "warn".to_owned(),
                        message: warning.clone(),
                        fields: serde_json::Map::new(),
                    }),
                )?;
            }
            if !output.text.is_empty() {
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
        } if kind.is_none() || title.is_none() => {
            // Kind first: it frames what you're about to write. The title is
            // read out of the composed text, or asked for last.
            let kind = match kind {
                Some(kind) => kind,
                None => pick_kind(stdin, stdout)?,
            };
            // A description given on the command line is a description, not a
            // draft of the whole ticket. Composing from it would read its one
            // line back as a title and file the ticket with no description at
            // all, quietly turning an explicit `--body` into something else.
            let (title, body) = match (title, body) {
                (Some(title), body) => (title, body),
                (None, Some(body)) => (ask_title(stdin, stdout)?, Some(body)),
                (None, None) => compose_ticket(Some(kind), stdin, stdout)?,
            };

            Ok(Command::Add {
                kind: Some(kind),
                title: Some(title),
                author,
                body,
                implements,
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

        Command::Show { id: None, json } => Ok(Command::Show {
            id: Some(pick_ticket(dir, stdin, stdout, "Show", false)?),
            json,
        }),

        Command::Edit {
            id: None,
            title,
            body,
            kind,
            status,
        } => Ok(Command::Edit {
            id: Some(pick_ticket(dir, stdin, stdout, "Edit", false)?),
            title,
            body,
            kind,
            status,
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

    let new = done.new.to_string();
    let mut ambiguous = vec![];

    // A ticket names itself nowhere structurally, so anything left inside the
    // renamed file is prose — and prose naming the old id is ambiguous the same
    // way any other file's is: it may mean the ticket that kept the id.
    //
    // The rename put this file here a line ago, so a read failure is not one of
    // the expected cases `rewrite` tolerates.
    match std::fs::read_to_string(&done.path) {
        Ok(source) if source.contains(&done.old) => ambiguous.push(done.path.clone()),
        Ok(_) => {}
        Err(error) => return Err(format!("{}: {error}", done.path)),
    }

    for file in changed.iter().chain([&dir.join(".board.json")]) {
        if *file == path {
            continue;
        }
        if names_on_base(dir, base, file, &done.old) {
            ambiguous.push(file.clone());
            continue;
        }
        if rewrite(file, &done.old, &new)? {
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
            "{} already appeared in these files on {base}, so the occurrence may belong to the \
             ticket that kept the id. Left alone for you to sort out: {names}.",
            done.old
        ));
    }

    Ok(output)
}

/// Convert every ticket left in the pre-RFD-102 format.
///
/// Each one lands in the bucket of the commit that added it, so a branch's
/// tickets keep their order against those already on `main`.
fn migrate(dir: &Utf8Path) -> Result<Output, String> {
    let entries = std::fs::read_dir(dir).map_err(|error| error.to_string())?;

    let mut legacy: Vec<Utf8PathBuf> = vec![];
    for entry in entries {
        let path = entry.map_err(|error| error.to_string())?.path();
        let Ok(path) = Utf8PathBuf::try_from(path) else {
            continue;
        };
        if path.file_name().is_some_and(legacy_filename) && path.extension() == Some("md") {
            legacy.push(path);
        }
    }
    legacy.sort();

    if legacy.is_empty() {
        return Ok("No tickets to migrate.\n".to_owned().into());
    }

    let mut output = Output::default();
    let mut renamed = vec![];
    for path in &legacy {
        let bucket = created_bucket(dir, path)?;
        let done = store::reassign(dir, path, bucket).map_err(|error| error.to_string())?;

        // A pre-RFD-102 ticket embedded its id in the heading and in reply
        // targets. `reassign` only renames, so those are converted here — and
        // before the cross-file rewrite below, which would otherwise turn a
        // reply into `T-<new>#1` and leave the ticket naming itself again.
        let source = std::fs::read_to_string(&done.path).map_err(|error| error.to_string())?;
        let converted = render::strip_ids(&source, &done.old);
        if converted != source {
            std::fs::write(&done.path, converted).map_err(|error| error.to_string())?;
        }

        output
            .text
            .push_str(&format!("{} -> {} at {}\n", done.old, done.new, done.path));
        renamed.push((done.old, done.new.to_string()));
    }

    // References are rewritten after every rename, so a ticket that names
    // another one is fixed whichever order they were converted in.
    let mut targets: Vec<Utf8PathBuf> = store::list(dir)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|entry| entry.path)
        .collect();
    targets.push(dir.join(".board.json"));

    for target in targets {
        for (old, new) in &renamed {
            if rewrite(&target, old, new)? {
                output
                    .text
                    .push_str(&format!("  rewrote {old} in {target}\n"));
            }
        }
    }

    output.warnings.push(
        "References outside `docs/ticket/` were not touched. Commit messages naming the old ids \
         stay dangling."
            .to_owned(),
    );

    Ok(output)
}

/// Whether `name` opens with the pre-RFD-102 `NNNN-` id.
///
/// Indexed through the bytes rather than by slicing the string: a name whose
/// fourth byte falls mid-character, `aéé.md`, would panic a byte range.
fn legacy_filename(name: &str) -> bool {
    let bytes = name.as_bytes();

    bytes
        .get(..4)
        .is_some_and(|id| id.iter().all(u8::is_ascii_digit))
        && bytes.get(4) == Some(&b'-')
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

/// Resolve the ticket directory against the workspace root.
///
/// An absolute `--dir` is taken as given; a relative one, and the default, hang
/// off the root so the commands behave the same from any subdirectory.
fn resolve_dir(root: &Utf8Path, dir: Option<&Utf8Path>) -> Utf8PathBuf {
    match dir {
        Some(dir) if dir.is_absolute() => dir.to_path_buf(),
        Some(dir) => root.join(dir),
        None => root.join(store::DEFAULT_DIR),
    }
}

fn execute(dir: &Utf8Path, command: Command, config: &Value) -> Result<Output, String> {
    match command {
        Command::Add {
            kind,
            title,
            author,
            body,
            implements,
        } => {
            let kind = required(kind, "kind")?;
            let title = required(title, "title")?;
            let author = resolve_author(author, config)?;
            let date = Local::now().format("%Y-%m-%d").to_string();
            let description = body.unwrap_or_default();
            let (id, path) = store::create(
                dir,
                kind,
                &title,
                &author,
                &date,
                implements.as_deref(),
                &description,
            )
            .map_err(|error| error.to_string())?;

            Ok(format!("Created {path} ({id})\n").into())
        }

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

        Command::Close { id } => {
            let id = required(id, "ticket id")?;
            let (path, previous) = store::close(dir, id).map_err(|error| error.to_string())?;

            Ok(match previous {
                Status::Done => format!("{path}: already Done\n"),
                previous => format!("{path}: {previous} -> Done\n"),
            }
            .into())
        }

        Command::Show { id, json } => show(dir, required(id, "ticket id")?, json),

        Command::Edit {
            id,
            title,
            body,
            kind,
            status,
        } => edit(
            dir,
            required(id, "ticket id")?,
            title.as_deref(),
            body.as_deref(),
            kind,
            status,
        ),

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

        Command::Migrate => migrate(dir),

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

        Command::List { status, kind, json } => list(dir, status, kind, json),
    }
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

/// Apply an edit, touching only the parts the caller named.
fn edit(
    dir: &Utf8Path,
    id: TicketId,
    title: Option<&str>,
    body: Option<&str>,
    kind: Option<Kind>,
    status: Option<Status>,
) -> Result<Output, String> {
    let failed = |error: store::Error| error.to_string();

    let mut path = if title.is_some() || body.is_some() {
        store::edit(dir, id, title, body).map_err(failed)?
    } else {
        store::locate_ticket(dir, id).map_err(failed)?
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

        return Ok(format!("{json}\n").into());
    }

    // The rendered view names the ticket even though the file doesn't: the id
    // is what a reply or a `Blocked by` has to quote.
    let mut out = format!("# {}: {}\n\n", entry.id, ticket.title);
    out.push_str(&format!("- **Path**: {}\n", entry.path));
    out.push_str(&format!("- **Status**: {}\n", ticket.metadata.status));
    out.push_str(&format!("- **Kind**: {}\n", ticket.metadata.kind));
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

fn list(
    dir: &Utf8Path,
    status: Option<Status>,
    kind: Option<Kind>,
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
    tickets.retain(|(_, ticket, _)| {
        status.is_none_or(|status| status == ticket.metadata.status)
            && kind.is_none_or(|kind| kind == ticket.metadata.kind)
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

    Ok(Output { text, warnings })
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

    format!("{id:<9} {status:<12} {kind:<8} {}{blocked}\n", ticket.title)
}

fn help_text() -> String {
    Args::command().render_help().to_string()
}

fn send_describe(stdout: &mut impl Write) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Describe(DescribeResponse {
            name: "ticket".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Track work items as markdown files".to_owned(),
            command: vec!["ticket".to_owned()],
            author: Some("Jean Mertz <git@jeanmertz.com>".to_owned()),
            help: Some(help_text()),
            repository: Some("https://github.com/dcdpr/jp".to_owned()),
        }),
    )
}

fn print(stdout: &mut impl Write, text: &str) -> Result<(), String> {
    send(
        stdout,
        &PluginToHost::Print(PrintMessage {
            text: text.to_owned(),
            channel: "content".into(),
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
#[path = "main_tests.rs"]
mod tests;
