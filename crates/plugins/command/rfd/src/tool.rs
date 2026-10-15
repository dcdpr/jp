//! The tools this plugin serves, answered when the host's `init` carries a tool
//! call.
//!
//! The host names a tool the way its `source` does, without the `rfd_` prefix
//! the assistant knows it by: `list` and `show`.
//! Each call runs once and ends with its outcome.
//! The host closed stdin after `init`, so nothing here asks it anything, and a
//! missing argument is an error naming the argument.
//!
//! The call's access policy is enforced before any tool runs: the RFD
//! directory, and the ticket directory when there is one, have to be inside the
//! workspace and readable under the policy.

use std::error;

use camino::{Utf8Path, Utf8PathBuf};
use jp_plugin::message::{InitMessage, ToolAction, ToolCall};
use jp_tool::{AccessPolicy, Action, Context, Outcome};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::{
    board::{self, Board},
    id::{RfdId, resolve},
    options::{Options, ticket_dir},
    render::{self, Sections},
};

type Error = Box<dyn error::Error + Send + Sync>;

/// What a tool answers: its outcome, or a failure that becomes an error outcome
/// on the way out.
type Answer = Result<Outcome, Error>;

/// The tools this plugin serves, by the name a `source` gives them.
pub(crate) const TOOLS: &[&str] = &["list", "show"];

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
    let loaded = match board::load(place.root, &place.rfds, &place.tickets) {
        Ok(loaded) => loaded,
        Err(refusal) => return refuse(refusal),
    };
    let args = Arguments(&call.arguments);

    match call.name.as_str() {
        "list" => list(&loaded.board, &args),
        "show" => show(&loaded.board, &place, &args),
        // `TOOLS` above holds exactly the names matched here.
        other => refuse(format!("`jp rfd` serves no tool named `{other}`.")),
    }
}

/// Where a call reads, once its access policy allowed it.
struct Place<'a> {
    /// The workspace root, which paths in results are relative to.
    root: &'a Utf8Path,

    /// The RFD directory.
    rfds: Utf8PathBuf,

    /// The ticket directory, which says what is in development.
    tickets: Utf8PathBuf,
}

impl<'a> Place<'a> {
    /// Check the call may read the RFD directory, and the ticket directory if
    /// it exists.
    ///
    /// A policy the plugin can't read refuses the call rather than reading as
    /// unrestricted, and a directory outside the workspace is refused whatever
    /// the policy says.
    /// The error is the refusal, for the assistant.
    fn open(init: &'a InitMessage, call: &ToolCall, options: &Options) -> Result<Self, String> {
        let root = init.workspace.root.as_path();
        let rfds = workspace_relative(root, &options.dir, "RFD")?;
        let tickets = workspace_relative(root, &ticket_dir(&init.config)?, "ticket")?;

        let access = call
            .access
            .clone()
            .map(serde_json::from_value::<AccessPolicy>)
            .transpose()
            .map_err(|error| format!("The access policy for this call can't be read: {error}"))?;

        let ctx = Context {
            root: root.to_owned(),
            action: Action::Run,
            access,
            workspace_id: init.workspace.id.clone(),
            conversation_id: call.conversation.clone(),
        };

        ctx.check_read(&rfds)
            .map_err(|error| format!("`{}` can't read the RFD directory: {error}", call.name))?;
        if root.join(&tickets).is_dir() {
            ctx.check_read(&tickets).map_err(|error| {
                format!("`{}` can't read the ticket directory: {error}", call.name)
            })?;
        }

        Ok(Self {
            root,
            rfds: root.join(rfds),
            tickets: root.join(tickets),
        })
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

/// List the board, the way `jp rfd list` prints it.
fn list(board: &Board, args: &Arguments<'_>) -> Answer {
    let category = args.optional::<String>("category")?;
    let sections = Sections::from_flags(
        args.flag("planned")?,
        args.flag("backlog")?,
        args.flag("all")?,
    );
    let full = args.flag("full")?;

    Ok(render::list_text(board, category.as_deref(), sections, full).into())
}

/// Show one RFD, the way `jp rfd show` prints it.
fn show(board: &Board, place: &Place<'_>, args: &Arguments<'_>) -> Answer {
    // A model passes `116` as a number as readily as a string.
    let id = match args.required::<Value>("id")? {
        Value::String(id) => id,
        Value::Number(number) => number.to_string(),
        other => return refuse(format!("`{other}` is not an RFD id.")),
    };
    let id = match id.parse::<RfdId>() {
        Ok(id) => id,
        Err(error) => return refuse(error.to_string()),
    };
    let file = match resolve(&place.rfds, id) {
        Ok(file) => board::relative(place.root, &file),
        Err(error) => return refuse(error.to_string()),
    };

    let Some(entry) = board.entry(&file) else {
        return refuse(format!("{file} is not an RFD: its name has no slug."));
    };

    let shown = render::show_text(board, entry);
    // The terminal keys syntax highlighting off a leading code fence, so the
    // fence is what gets the RFD displayed as markdown rather than as a flat
    // wall of text.
    Ok(format!("```markdown\n{}\n```", shown.trim_end()).into())
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
