//! The tools this plugin serves, answered when the host's `init` carries a tool
//! call.
//!
//! The host names a tool the way its `source` does, without the `rfd_` prefix
//! the assistant knows it by: `list`, `show`, `draft`, and so on.
//! Each call runs once and ends with its outcome.
//! The host closed stdin after `init`, so nothing here asks it anything, and a
//! missing argument is an error naming the argument.
//!
//! The call's access policy is enforced before a tool reads or writes: the RFD
//! directory, and the ticket directory when the tool touches tickets, have to
//! be inside the workspace and allow what the tool does to them.
//!
//! With nothing to ask, `promote` offers no tracking ticket, and `track` and
//! `start` file one without reading the plan: its marker stays in the
//! description until `jp rfd track` runs where the host can read it.

use std::error;

use camino::{Utf8Path, Utf8PathBuf};
use jp_plugin::message::{InitMessage, ToolAction, ToolCall};
use jp_tool::{AccessPolicy, Action, Capability, Context, FsAccessError, Outcome};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use ticket::TicketId;

use crate::{
    Output, author,
    board::{self, Board},
    cmd::{
        self,
        draft::Draft,
        track::{Filer, Plan},
    },
    document::Document,
    id::RfdId,
    options::{self, Options, ticket_dir},
    paths::{self, Paths},
    render::{self, Sections},
    today,
};

type Error = Box<dyn error::Error + Send + Sync>;

/// What a tool answers: its outcome, or a failure that becomes an error outcome
/// on the way out.
type Answer = Result<Outcome, Error>;

/// The tools this plugin serves, by the name a `source` gives them, sorted.
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
    "start",
    "stop",
    "supersede",
    "track",
];

/// Answer one tool call.
///
/// Every failure becomes an error outcome, so the assistant reads what went
/// wrong and can correct the call.
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
        "draft" => draft(&place, &args, options, &init.config),
        "promote" => promote(&place, &args),
        "extend" => relate(&place, &args, "target", cmd::relate::extend),
        "require" => relate(&place, &args, "target", cmd::relate::require),
        "supersede" => relate(&place, &args, "by", cmd::relate::supersede),
        "abandon" => abandon(&place, &args),
        "renumber" => renumber(&place, &args),
        "rename" => rename(&place, &args),
        "track" => track(&place, &args, &init.config),
        "start" => start(&place, &args, &init.config),
        "stop" => stop(&place, &args),
        // `TOOLS` above holds exactly the names matched here.
        other => Err(Refusal(format!("`jp rfd` serves no tool named `{other}`."))),
    };

    match result {
        Ok(outcome) => Ok(outcome),
        Err(Refusal(message)) => refuse(message),
    }
}

/// A call that can't go ahead, with what the assistant can do about it.
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

/// What a tool answers before the refusal is turned into an outcome.
type Reply = Result<Outcome, Refusal>;

/// A directory a tool works in, for the access policy.
#[derive(Debug, Clone, Copy)]
enum Dir {
    Rfds,
    Tickets,
}

/// Where a call works, and the access policy it works under.
struct Place<'a> {
    name: &'a str,
    paths: Paths,

    /// The RFD directory, relative to the workspace root, as the policy reads
    /// paths.
    rfds: Utf8PathBuf,

    /// The ticket directory, likewise.
    tickets: Utf8PathBuf,

    ctx: Context,
}

impl<'a> Place<'a> {
    /// Resolve the directories a call works in and read its access policy.
    ///
    /// A policy the plugin can't read refuses the call rather than reading as
    /// unrestricted, and a directory outside the workspace is refused whatever
    /// the policy says.
    /// The error is the refusal, for the assistant.
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
            paths: Paths::new(root, &rfds, &tickets),
            rfds,
            tickets,
            ctx: Context {
                root: root.to_owned(),
                action: Action::Run,
                access,
                workspace_id: init.workspace.id.clone(),
                conversation_id: call.conversation.clone(),
            },
        })
    }

    /// Refuse the call unless the policy allows each of `capabilities` on
    /// `dir`.
    fn require(&self, dir: Dir, capabilities: &[Capability]) -> Result<(), String> {
        let (path, what) = match dir {
            Dir::Rfds => (&self.rfds, "RFD"),
            Dir::Tickets => (&self.tickets, "ticket"),
        };

        self.require_at(path, what, capabilities)
    }

    /// Refuse the call unless the policy allows each of `capabilities` on the
    /// workspace-relative `path`, the `what` directory.
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

    /// Like [`Self::require`] for the ticket directory, when there is one.
    fn require_tickets(&self, capabilities: &[Capability]) -> Result<(), String> {
        if self.paths.tickets.is_dir() {
            self.require(Dir::Tickets, capabilities)?;
        }

        Ok(())
    }

    /// Whether the policy lets the call search the whole workspace, which
    /// `renumber` and `rename` do for references they leave behind.
    fn reads_workspace(&self) -> bool {
        self.ctx.check_read(Utf8Path::new(".")).is_ok()
    }

    /// Read the board, the way the reading tools start.
    fn board(&self) -> Result<Board, String> {
        board::load(&self.paths.root, &self.paths.rfds, &self.paths.tickets)
            .map(|loaded| loaded.board)
    }
}

/// A directory as the access policy reads paths: relative to the workspace
/// root.
///
/// One configured as an absolute path inside the workspace is taken relative to
/// it; one outside it is refused, since a tool reaches only the workspace.
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

/// Ask the access policy about one capability on `path`.
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

/// A call's arguments, read with errors that name the argument.
struct Arguments<'a>(&'a Map<String, Value>);

impl Arguments<'_> {
    /// An argument the call can't do without.
    ///
    /// Absent and `null` are both missing.
    fn required<T: DeserializeOwned>(&self, key: &str) -> Result<T, Error> {
        match self.0.get(key) {
            None | Some(Value::Null) => Err(format!("Missing argument `{key}`.").into()),
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|error| format!("Argument `{key}` is unusable: {error}.").into()),
        }
    }

    /// An argument the call can do without; absent and `null` are both `None`.
    fn optional<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, Error> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(_) => self.required(key).map(Some),
        }
    }

    /// A flag, `false` when absent.
    fn flag(&self, key: &str) -> Result<bool, Error> {
        Ok(self.optional(key)?.unwrap_or(false))
    }

    /// An argument holding an RFD id, as a string or, the way a model passes
    /// `116` as readily, a number.
    fn id(&self, key: &str) -> Result<RfdId, Refusal> {
        let text = self.id_text(key)?;
        text.parse::<RfdId>()
            .map_err(|error| Refusal(error.to_string()))
    }

    /// The text of an id argument, unparsed.
    fn id_text(&self, key: &str) -> Result<String, Refusal> {
        match self.required::<Value>(key)? {
            Value::String(id) => Ok(id),
            Value::Number(number) => Ok(number.to_string()),
            other => Err(Refusal(format!("`{other}` is not an RFD id."))),
        }
    }
}

/// End the call with a message the assistant can act on.
#[expect(clippy::unnecessary_wraps, reason = "every tool returns an `Answer`")]
fn refuse(message: impl Into<String>) -> Answer {
    Ok(Outcome::Error {
        message: message.into(),
        trace: vec![],
        transient: true,
    })
}

/// What a command reported, as the assistant reads it: what it did, what
/// changed, then the warnings about what is left to check.
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

/// Add `line` to `text` on a line of its own.
fn push_line(text: &mut String, line: &str) {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
}

/// List the board, the way `jp rfd list` prints it.
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

    let board = place.board()?;
    Ok(render::list_text(&board, category.as_deref(), sections, full).into())
}

/// Show one RFD, the way `jp rfd show` prints it.
fn show(place: &Place<'_>, args: &Arguments<'_>) -> Reply {
    place.require(Dir::Rfds, &[Capability::Read])?;
    place.require_tickets(&[Capability::Read])?;

    let id = args.id("id")?;
    let board = place.board()?;
    let file = place.paths.show(&place.paths.resolve(id)?);
    let Some(entry) = board.entry(&file) else {
        return Err(Refusal(format!(
            "{file} is not an RFD: its name has no slug."
        )));
    };

    let shown = render::show_text(&board, entry);
    // The terminal keys syntax highlighting off a leading code fence, so the
    // fence is what gets the RFD displayed as markdown rather than as a flat
    // wall of text.
    Ok(format!("```markdown\n{}\n```", shown.trim_end()).into())
}

/// Write a new draft, from a ticket when the call names one.
fn draft(place: &Place<'_>, args: &Arguments<'_>, options: &Options, config: &Value) -> Reply {
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

    let author = author::resolve(&place.paths.root, config)?;
    let date = today();
    let request = Draft {
        category: &category,
        title: &title,
        author: &author,
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

/// Advance an RFD one status.
///
/// What the call has to be allowed depends on the step, which the RFD's status
/// says: publishing a draft moves its file and rewrites the references to it,
/// tickets included.
fn promote(place: &Place<'_>, args: &Arguments<'_>) -> Reply {
    let id = args.id("id")?;
    place.require(Dir::Rfds, &[Capability::Read, Capability::Update])?;

    let file = place.paths.resolve(id)?;
    let status = Document::parse(&paths::read(&file)?).header().status();
    if status.as_deref() == Some("Draft") {
        place.require(Dir::Rfds, &[Capability::Create, Capability::Delete])?;
        place.require_tickets(&[Capability::Read, Capability::Update])?;
    }

    let mut output = cmd::promote::promote(&place.paths, id)?;
    if status.as_deref() == Some("Discussion") && place.paths.tickets.is_dir() {
        output.text.push_str(&format!(
            "Skipped the tracking ticket, which a tool call doesn't offer. File it with `jp rfd \
             track {id}` when the work starts.\n"
        ));
    }

    Ok(reported(output))
}

/// Record a relationship between the RFD `id` names and the one `other` names.
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

/// The capabilities moving an RFD's file takes: one name goes, another is
/// created, and the references to it are rewritten.
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

    let output =
        cmd::renumber::renumber(&place.paths, &id, new.as_deref(), place.reads_workspace())?;

    Ok(reported(output))
}

fn rename(place: &Place<'_>, args: &Arguments<'_>) -> Reply {
    let id = args.id("id")?;
    let title = args.required::<String>("title")?;
    place.require(Dir::Rfds, &MOVE)?;
    // Links name the file, so they are rewritten wherever they are.
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

/// Who a tracking ticket a tool files is attributed to, filing today.
fn assistant(config: &Value) -> Result<Filer, Refusal> {
    Ok(Filer {
        author: options::assistant(config)?,
        date: today(),
    })
}

/// The plan a tool call reads: none, since the host can't be asked to run a
/// query once the call has started.
#[expect(
    clippy::unnecessary_wraps,
    reason = "`track` takes a plan reader that can fail"
)]
fn no_plan(_: &cmd::track::PlanRequest<'_>) -> Result<Plan, String> {
    Ok(Plan::Unavailable(
        "a tool call can't ask the host to run a query".to_owned(),
    ))
}

/// What filing or moving a tracking ticket takes in the ticket directory.
const TRACKING: [Capability; 3] = [Capability::Read, Capability::Create, Capability::Update];

fn track(place: &Place<'_>, args: &Arguments<'_>, config: &Value) -> Reply {
    let id = args.id("id")?;
    place.require(Dir::Rfds, &[Capability::Read])?;
    place.require_tickets(&TRACKING)?;

    let tracked = cmd::track::track(&place.paths, id, &assistant(config)?, &mut no_plan)?;

    Ok(reported(tracked.report))
}

fn start(place: &Place<'_>, args: &Arguments<'_>, config: &Value) -> Reply {
    let id = args.id("id")?;
    place.require(Dir::Rfds, &[Capability::Read])?;
    place.require_tickets(&TRACKING)?;

    let output = cmd::track::start(&place.paths, id, &assistant(config)?, &mut no_plan)?;

    Ok(reported(output))
}

fn stop(place: &Place<'_>, args: &Arguments<'_>) -> Reply {
    let id = args.id("id")?;
    place.require(Dir::Rfds, &[Capability::Read])?;
    place.require_tickets(&[Capability::Read, Capability::Update])?;

    Ok(reported(cmd::track::stop(&place.paths, id)?))
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
