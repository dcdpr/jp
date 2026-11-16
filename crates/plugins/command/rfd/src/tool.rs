//! Tool call dispatch for `jp-rfd`, invoked when `init` carries a tool call.
//!
//! The host passes the tool name from `source` without the `rfd_` prefix (for
//! example, `list`, `show`, or `draft`).
//! The host closes stdin after `init`, so tools cannot prompt interactively;
//! missing arguments return an error naming the argument.
//!
//! Each tool checks the call's access policy before reading or writing: the RFD
//! directory (and the ticket directory when accessed) must be inside the
//! workspace and grant the required capabilities.
//!
//! RFD tracking is managed through the ticket tools: `show` and `promote` print
//! the labels required to file a tracking ticket with `ticket_create`, and
//! `ticket_edit` moves the ticket to start or stop work.
//! Because tool calls cannot prompt interactively, `promote` skips the tracking
//! ticket prompt.

use std::error;

use camino::{Utf8Path, Utf8PathBuf};
use jp_plugin::message::{InitMessage, ToolAction, ToolCall};
use jp_tool::{AccessPolicy, Action, Capability, Context, FsAccessError, Outcome};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use ticket::TicketId;

use crate::{
    Output,
    board::{self, Loaded},
    cmd::{self, draft::Draft},
    document::{self, Document},
    id::RfdId,
    links::Links,
    options::{Options, ticket_dir},
    paths::{self, Paths},
    render::{self, Sections},
    today, untracked,
};

type Error = Box<dyn error::Error + Send + Sync>;

type Answer = Result<Outcome, Error>;

/// Tool names served by this plugin, without the `rfd_` prefix, in sorted
/// order.
pub(crate) const TOOLS: &[&str] = &[
    "abandon",
    "draft",
    "extend",
    "list",
    "promote",
    "rename",
    "renumber",
    "require",
    "show",
    "supersede",
];

/// Execute a tool call and convert any failure into an error [`Outcome`].
pub(crate) fn run(init: &InitMessage, call: &ToolCall, options: &Options) -> Outcome {
    match answer(init, call, options) {
        Ok(outcome) => outcome,
        Err(error) => Outcome::error(error.as_ref()),
    }
}

fn answer(init: &InitMessage, call: &ToolCall, options: &Options) -> Answer {
    if !TOOLS.contains(&call.name.as_str()) {
        return refuse(format!(
            "`jp rfd` serves no tool named `{}`. It serves: {}.",
            call.name,
            TOOLS.join(", ")
        ));
    }

    if call.action == ToolAction::FormatArguments {
        return refuse(format!(
            "`{}` has no preview of its own; style its parameters as `function_call` or `json` \
             rather than `tool`.",
            call.name
        ));
    }

    let place = match Place::open(init, call, options) {
        Ok(place) => place,
        Err(refusal) => return refuse(refusal),
    };
    let args = Arguments(&call.arguments);

    let result = match call.name.as_str() {
        "list" => list(&place, &args),
        "show" => show(&place, &args),
        "draft" => draft(&place, &args, options),
        "promote" => promote(&place, &args),
        "extend" => relate(&place, &args, "target", cmd::relate::extend),
        "require" => relate(&place, &args, "target", cmd::relate::require),
        "supersede" => relate(&place, &args, "by", cmd::relate::supersede),
        "abandon" => abandon(&place, &args),
        "renumber" => renumber(&place, &args),
        "rename" => rename(&place, &args),
        // `TOOLS` above holds exactly the names matched here.
        other => Err(Refusal(format!("`jp rfd` serves no tool named `{other}`."))),
    };

    match result {
        Ok(outcome) => Ok(outcome),
        Err(Refusal(message)) => refuse(message),
    }
}

/// Tool execution failure with a message returned to the assistant.
struct Refusal(String);

impl From<String> for Refusal {
    fn from(message: String) -> Self {
        Self(message)
    }
}

impl From<Error> for Refusal {
    fn from(error: Error) -> Self {
        Self(error.to_string())
    }
}

type Reply = Result<Outcome, Refusal>;

/// Workspace directory checked against the tool's access policy.
#[derive(Debug, Clone, Copy)]
enum Dir {
    Rfds,
    Tickets,
}

/// Resolved workspace paths and access policy context for a tool call.
struct Place<'a> {
    name: &'a str,
    paths: Paths,

    /// RFD directory relative to the workspace root, for access policy checks.
    rfds: Utf8PathBuf,

    /// Ticket directory relative to the workspace root, for access policy
    /// checks.
    tickets: Utf8PathBuf,

    /// Label keys used to link tickets to RFDs.
    links: Links,

    ctx: Context,
}

impl<'a> Place<'a> {
    /// Resolve the directories for a tool call and parse its access policy.
    ///
    /// Rejects unreadable access policies and directories outside the
    /// workspace.
    fn open(init: &InitMessage, call: &'a ToolCall, options: &Options) -> Result<Self, String> {
        let root = init.workspace.root.as_path();
        let rfds = workspace_relative(root, &options.dir, "RFD")?;
        let tickets = workspace_relative(root, &ticket_dir(&init.config)?, "ticket")?;

        let access = call
            .access
            .clone()
            .map(serde_json::from_value::<AccessPolicy>)
            .transpose()
            .map_err(|error| format!("The access policy for this call can't be read: {error}"))?;

        Ok(Self {
            name: &call.name,
            paths: Paths::new(root, &rfds, &tickets).formatted(&init.config)?,
            rfds,
            tickets,
            links: options.links.clone(),
            ctx: Context {
                root: root.to_owned(),
                action: Action::Run,
                access,
                workspace_id: init.workspace.id.clone(),
                conversation_id: call.conversation.clone(),
            },
        })
    }

    /// Reject the call unless the access policy grants all `capabilities` on
    /// `dir`.
    fn require(&self, dir: Dir, capabilities: &[Capability]) -> Result<(), String> {
        let (path, what) = match dir {
            Dir::Rfds => (&self.rfds, "RFD"),
            Dir::Tickets => (&self.tickets, "ticket"),
        };

        self.require_at(path, what, capabilities)
    }

    /// Reject the call unless the access policy grants all `capabilities` on
    /// the workspace-relative `path`.
    fn require_at(
        &self,
        path: &Utf8Path,
        what: &str,
        capabilities: &[Capability],
    ) -> Result<(), String> {
        for capability in capabilities {
            check(&self.ctx, path, *capability).map_err(|error| {
                format!("`{}` can't reach the {what} directory: {error}", self.name)
            })?;
        }

        Ok(())
    }

    /// Check `capabilities` on the ticket directory if it exists.
    fn require_tickets(&self, capabilities: &[Capability]) -> Result<(), String> {
        if self.paths.tickets.is_dir() {
            self.require(Dir::Tickets, capabilities)?;
        }

        Ok(())
    }

    /// Return whether the access policy allows reading the workspace root.
    fn reads_workspace(&self) -> bool {
        self.ctx.check_read(Utf8Path::new(".")).is_ok()
    }

    /// Load the RFD board and its linked tickets.
    fn board(&self) -> Result<Loaded, String> {
        board::load(
            &self.paths.root,
            &self.paths.rfds,
            Some((&self.paths.tickets, &self.links)),
        )
    }
}

/// Convert `dir` to a path relative to the workspace `root`.
///
/// Absolute paths inside `root` are stripped to relative paths; paths outside
/// `root` are rejected because tools can only access the workspace.
fn workspace_relative(root: &Utf8Path, dir: &Utf8Path, what: &str) -> Result<Utf8PathBuf, String> {
    if !dir.is_absolute() {
        return Ok(dir.to_path_buf());
    }

    dir.strip_prefix(root)
        .map(Utf8Path::to_path_buf)
        .map_err(|_| {
            format!(
                "The {what} directory {dir} is outside the workspace, which is all a tool reaches."
            )
        })
}

fn check(ctx: &Context, path: &Utf8Path, capability: Capability) -> Result<(), FsAccessError> {
    match capability {
        Capability::Read => ctx.check_read(path),
        Capability::Create => ctx.check_create(path),
        Capability::Update => ctx.check_update(path),
        Capability::Delete => ctx.check_delete(path),
        Capability::Execute => ctx.check_execute(path),
    }
    .map(drop)
}

/// Tool call arguments with error messages that name the missing or invalid
/// key.
struct Arguments<'a>(&'a Map<String, Value>);

impl Arguments<'_> {
    /// Deserialize a required argument, treating absent or `null` values as
    /// missing.
    fn required<T: DeserializeOwned>(&self, key: &str) -> Result<T, Error> {
        match self.0.get(key) {
            None | Some(Value::Null) => Err(format!("Missing argument `{key}`.").into()),
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|error| format!("Argument `{key}` is unusable: {error}.").into()),
        }
    }

    /// Deserialize an optional argument, treating absent or `null` values as
    /// `None`.
    fn optional<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, Error> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(_) => self.required(key).map(Some),
        }
    }

    /// Deserialize a boolean flag, defaulting to `false` when absent.
    fn flag(&self, key: &str) -> Result<bool, Error> {
        Ok(self.optional(key)?.unwrap_or(false))
    }

    /// Parse an RFD ID argument from a JSON string or integer.
    fn id(&self, key: &str) -> Result<RfdId, Refusal> {
        let text = self.id_text(key)?;
        text.parse::<RfdId>()
            .map_err(|error| Refusal(error.to_string()))
    }

    /// Extract an RFD ID argument as a raw string from a JSON string or
    /// integer.
    fn id_text(&self, key: &str) -> Result<String, Refusal> {
        match self.required::<Value>(key)? {
            Value::String(id) => Ok(id),
            Value::Number(number) => Ok(number.to_string()),
            other => Err(Refusal(format!("`{other}` is not an RFD id."))),
        }
    }
}

/// Return a transient error [`Outcome`] with `message`.
#[expect(clippy::unnecessary_wraps, reason = "every tool returns an `Answer`")]
fn refuse(message: impl Into<String>) -> Answer {
    Ok(Outcome::Error {
        message: message.into(),
        trace: vec![],
        transient: true,
    })
}

/// Format command [`Output`] into a tool [`Outcome`]: notes, text, then
/// warnings.
fn reported(output: Output) -> Outcome {
    let mut text = String::new();
    for note in output.notes {
        push_line(&mut text, &note);
    }
    text.push_str(&output.text);
    for warning in output.warnings {
        push_line(&mut text, &warning);
    }

    text.into()
}

fn push_line(text: &mut String, line: &str) {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
}

fn list(place: &Place<'_>, args: &Arguments<'_>) -> Reply {
    place.require(Dir::Rfds, &[Capability::Read])?;
    place.require_tickets(&[Capability::Read])?;

    let category = args.optional::<String>("category")?;
    let sections = Sections::from_flags(
        args.flag("planned")?,
        args.flag("backlog")?,
        args.flag("all")?,
    );
    let full = args.flag("full")?;

    let Loaded { board, warnings } = place.board()?;

    Ok(reported(Output {
        text: render::list_text(&board, category.as_deref(), sections, full),
        warnings,
        ..Output::default()
    }))
}

fn show(place: &Place<'_>, args: &Arguments<'_>) -> Reply {
    place.require(Dir::Rfds, &[Capability::Read])?;
    place.require_tickets(&[Capability::Read])?;

    let id = args.id("id")?;
    let with_body = args.flag("body")?;
    let Loaded { board, warnings } = place.board()?;
    let path = place.paths.resolve(id)?;
    let file = place.paths.show(&path);
    let Some(entry) = board.entry(&file) else {
        return Err(Refusal(format!(
            "{file} is not an RFD: its name has no slug."
        )));
    };
    let body = if with_body {
        Some(document::body(&paths::read(&path)?))
    } else {
        None
    };

    let shown = render::show_text(&board, entry, body.as_deref());
    // Wrap the output in a markdown code fence so the terminal syntax-highlights
    // the RFD instead of rendering plain text.
    let mut text = fenced(shown.trim_end());
    if let Some(untracked) = untracked(&place.paths, &place.links, entry) {
        text.push_str(&format!("\n{untracked}"));
    }

    Ok(reported(Output {
        text,
        warnings,
        ..Output::default()
    }))
}

/// Wrap `text` in a markdown code fence longer than any backtick run inside it.
fn fenced(text: &str) -> String {
    let longest = text
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or_default();
    let fence = "`".repeat(longest.max(2) + 1);

    format!("{fence}markdown\n{text}\n{fence}")
}

/// Create a new draft (optionally seeded from a ticket) attributed to
/// `options.assistant`.
fn draft(place: &Place<'_>, args: &Arguments<'_>, options: &Options) -> Reply {
    let category = args.required::<String>("category")?;
    let ticket = args
        .optional::<String>("ticket")?
        .map(|ticket| ticket.parse::<TicketId>())
        .transpose()
        .map_err(|error| Refusal(error.to_string()))?;
    let title = match ticket {
        Some(_) => args.optional::<String>("title")?.unwrap_or_default(),
        None => args.required::<String>("title")?,
    };

    place.require(Dir::Rfds, &[Capability::Read, Capability::Create])?;
    if ticket.is_some() {
        place.require(Dir::Tickets, &[Capability::Read, Capability::Update])?;
    }

    let date = today();
    let request = Draft {
        category: &category,
        title: &title,
        author: &options.assistant,
        date: &date,
    };

    let output = match ticket {
        Some(ticket) => {
            cmd::draft::from_ticket(&place.paths, &options.categories, ticket, &request)?
        }
        None => cmd::draft::draft(&place.paths, &options.categories, &request)?,
    };

    Ok(reported(output))
}

/// Advance an RFD to its next status.
///
/// Promoting a draft moves its file and rewrites references in RFDs and
/// tickets, so it requires `Create` and `Delete` on the RFD directory and
/// `Update` on the ticket directory.
fn promote(place: &Place<'_>, args: &Arguments<'_>) -> Reply {
    let id = args.id("id")?;
    place.require(Dir::Rfds, &[Capability::Read, Capability::Update])?;

    let file = place.paths.resolve(id)?;
    let status = Document::parse(&paths::read(&file)?).header().status();
    if status.as_deref() == Some("Draft") {
        place.require(Dir::Rfds, &[Capability::Create, Capability::Delete])?;
        place.require_tickets(&[Capability::Read, Capability::Update])?;
    }

    let mut output = cmd::promote::promote(&place.paths, id, None)?;
    if status.as_deref() == Some("Discussion") && place.paths.tickets.is_dir() {
        output.text.push_str(&format!(
            "Skipped the tracking ticket, which a tool call doesn't offer. When the work starts, \
             file it as a ticket labelled {}, or with `jp rfd track {id}`.\n",
            place.links.describe_tracking(id)
        ));
    }

    Ok(reported(output))
}

/// Record a relationship between two RFDs.
fn relate(
    place: &Place<'_>,
    args: &Arguments<'_>,
    other: &str,
    write: fn(&Paths, RfdId, RfdId) -> Result<Output, String>,
) -> Reply {
    let id = args.id("id")?;
    let other = args.id(other)?;
    place.require(Dir::Rfds, &[Capability::Read, Capability::Update])?;

    Ok(reported(write(&place.paths, id, other)?))
}

fn abandon(place: &Place<'_>, args: &Arguments<'_>) -> Reply {
    let id = args.id("id")?;
    let reason = args.required::<String>("reason")?;
    place.require(Dir::Rfds, &[Capability::Read, Capability::Update])?;

    Ok(reported(cmd::abandon::abandon(&place.paths, id, &reason)?))
}

/// Capabilities required when moving an RFD file and rewriting references to
/// it.
const MOVE: [Capability; 4] = [
    Capability::Read,
    Capability::Create,
    Capability::Update,
    Capability::Delete,
];

fn renumber(place: &Place<'_>, args: &Arguments<'_>) -> Reply {
    let id = args.id_text("id")?;
    let new = match args.optional::<Value>("new")? {
        None => None,
        Some(Value::String(new)) => Some(new),
        Some(Value::Number(number)) => Some(number.to_string()),
        Some(other) => return Err(Refusal(format!("`{other}` is not an RFD id."))),
    };
    place.require(Dir::Rfds, &MOVE)?;
    place.require_tickets(&[Capability::Read, Capability::Update])?;

    let output = cmd::renumber::renumber(
        &place.paths,
        &place.links,
        &id,
        new.as_deref(),
        place.reads_workspace(),
    )?;

    Ok(reported(output))
}

fn rename(place: &Place<'_>, args: &Arguments<'_>) -> Reply {
    let id = args.id("id")?;
    let title = args.required::<String>("title")?;
    place.require(Dir::Rfds, &MOVE)?;
    // Renaming changes the file slug, so links in every documentation directory
    // must be rewritten.
    for dir in place.paths.linking() {
        let what = if dir == place.paths.tickets {
            "ticket"
        } else {
            "documentation"
        };
        let relative = dir
            .strip_prefix(&place.paths.root)
            .map_or_else(|_| dir.clone(), Utf8Path::to_path_buf);
        place.require_at(&relative, what, &[Capability::Read, Capability::Update])?;
    }

    let output = cmd::rename::rename(&place.paths, id, &title, place.reads_workspace())?;

    Ok(reported(output))
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
