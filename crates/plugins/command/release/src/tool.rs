//! The tool this plugin serves, answered when the host's `init` carries a tool
//! call.
//!
//! The host names it the way its `source` does, without the `changelog_` the
//! assistant knows it by: `add`, which writes a fragment.
//! A call runs once and ends with its outcome.
//! The host closed stdin after `init`, so nothing here asks it anything, and a
//! missing argument is an error naming the argument.
//! `add` also answers `format_arguments`, previewing the file it is about to
//! write.
//!
//! The call's access policy is enforced on the fragments directory before the
//! tool runs: the directory has to be inside the workspace, and the policy has
//! to let the tool read it and create a file in it.

use camino::{Utf8Path, Utf8PathBuf};
use changelog::Type;
use jp_md::format::Formatter;
use jp_plugin::message::{InitMessage, ToolAction, ToolCall};
use jp_tool::{AccessPolicy, Action, Context, Outcome};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::{
    entry::{self, Fragments},
    options::Options,
};

type Error = Box<dyn std::error::Error + Send + Sync>;

/// What a tool answers: its outcome, or a failure that becomes an error outcome
/// on the way out.
type Answer = Result<Outcome, Error>;

/// The tools this plugin serves, by the name a `source` gives them.
pub(crate) const TOOLS: &[&str] = &["add"];

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
            "`jp release` serves no tool named `{}`. It serves: {}.",
            call.name,
            TOOLS.join(", ")
        ));
    }

    let root = init.workspace.root.as_path();
    let relative = match workspace_relative(root, &options.notes.fragments_dir) {
        Ok(relative) => relative,
        Err(refusal) => return refuse(refusal),
    };
    if let Err(refusal) = check_access(init, call, &relative) {
        return refuse(refusal);
    }

    let args = Arguments(&call.arguments);
    let type_ = match args.required::<String>("type")?.parse::<Type>() {
        Ok(type_) => type_,
        Err(unknown) => return refuse(unknown.to_string()),
    };
    let body = args.required::<String>("body")?;
    let slug = args.optional::<String>("slug")?;

    let fragments = match Fragments::new(root, root.join(&relative), &init.config) {
        Ok(fragments) => fragments,
        Err(refusal) => return refuse(refusal),
    };
    // Planned before the action split, so a preview fails too: an unattended
    // preview that errors fails the call ahead of the approval prompt, which
    // tells the assistant to fix the arguments rather than asking the user
    // about a call that cannot land.
    let planned = match entry::plan(&fragments, type_, &body, slug.as_deref()) {
        Ok(planned) => planned,
        Err(refusal) => return refuse(refusal),
    };

    if call.action == ToolAction::FormatArguments {
        return Ok(preview(&planned.shown, &planned.text).into());
    }

    match planned.write() {
        Ok(()) => Ok(format!("Wrote {}", planned.shown).into()),
        Err(refusal) => refuse(refusal),
    }
}

/// The fragments directory as the access policy reads paths: relative to the
/// workspace root.
///
/// A directory configured as an absolute path inside the workspace is taken
/// relative to it; one outside it is refused, since a tool reaches only the
/// workspace.
fn workspace_relative(root: &Utf8Path, dir: &Utf8Path) -> Result<Utf8PathBuf, String> {
    if !dir.is_absolute() {
        return Ok(dir.to_path_buf());
    }

    dir.strip_prefix(root)
        .map(Utf8Path::to_path_buf)
        .map_err(|_| {
            format!(
                "The fragments directory {dir} is outside the workspace, which is all a tool \
                 reaches."
            )
        })
}

/// Check the call's access policy lets it read the fragments directory, `dir`,
/// and create a file in it.
///
/// A policy the plugin can't read refuses the call rather than reading as
/// unrestricted.
/// The error is the refusal, for the assistant.
fn check_access(init: &InitMessage, call: &ToolCall, dir: &Utf8Path) -> Result<(), String> {
    let access = call
        .access
        .clone()
        .map(serde_json::from_value::<AccessPolicy>)
        .transpose()
        .map_err(|error| format!("The access policy for this call can't be read: {error}"))?;

    let ctx = Context {
        root: init.workspace.root.clone(),
        action: match call.action {
            ToolAction::Run => Action::Run,
            ToolAction::FormatArguments => Action::FormatArguments,
        },
        access,
        workspace_id: init.workspace.id.clone(),
        conversation_id: call.conversation.clone(),
    };

    let unreachable = |error| {
        format!(
            "`{}` can't reach the fragments directory: {error}",
            call.name
        )
    };
    ctx.check_read(dir).map_err(unreachable)?;
    ctx.check_create(dir).map_err(unreachable)?;

    Ok(())
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

/// The file at `path` a call is about to write, holding `text`, styled for the
/// terminal as the user sees it before approving the call.
///
/// The file is shown whole in a code block, so a `Credits` field and an upgrade
/// section read as they are written rather than rendered.
/// Falls back to the unstyled source if the Markdown can't be formatted.
fn preview(path: &str, text: &str) -> String {
    let fence = "`".repeat(longest_backtick_run(text).max(2) + 1);
    let markdown = format!("Writes `{path}`:\n\n{fence}markdown\n{text}{fence}\n");

    Formatter::new()
        .format_terminal(&markdown)
        .unwrap_or(markdown)
}

/// The longest run of backticks in `text`, which a fence around it has to
/// outrun.
fn longest_backtick_run(text: &str) -> usize {
    text.split(|c| c != '`').map(str::len).max().unwrap_or(0)
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
