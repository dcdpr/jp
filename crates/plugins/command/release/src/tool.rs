//! Execution of the `add` tool (`source = "plugin.command.release.add"`).
//!
//! Handles both `run` (writing a new fragment file) and `format_arguments`
//! (rendering a terminal preview of the planned file before approval).
//! Before planning or writing, the tool verifies that the configured fragments
//! directory is inside the workspace and permitted by the call's
//! [`AccessPolicy`].

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
type Answer = Result<Outcome, Error>;

pub(crate) const TOOLS: &[&str] = &["add"];

/// Execute `call` and return its [`Outcome`].
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
    // Plan and validate before branching on `call.action` so invalid arguments
    // fail during `FormatArguments` before prompting the user for approval.
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

/// Resolve `dir` as a path relative to the workspace `root`, rejecting paths
/// outside the workspace.
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

/// Verify that `call`'s [`AccessPolicy`] grants read and create access to
/// `dir`.
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

struct Arguments<'a>(&'a Map<String, Value>);

impl Arguments<'_> {
    fn required<T: DeserializeOwned>(&self, key: &str) -> Result<T, Error> {
        match self.0.get(key) {
            None | Some(Value::Null) => Err(format!("Missing argument `{key}`.").into()),
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|error| format!("Argument `{key}` is unusable: {error}.").into()),
        }
    }

    fn optional<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, Error> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(_) => self.required(key).map(Some),
        }
    }
}

#[expect(clippy::unnecessary_wraps, reason = "every tool returns an `Answer`")]
fn refuse(message: impl Into<String>) -> Answer {
    Ok(Outcome::Error {
        message: message.into(),
        trace: vec![],
        transient: true,
    })
}

/// Render a terminal-styled Markdown preview of the fragment `text` to be
/// written at `path`.
fn preview(path: &str, text: &str) -> String {
    let fence = "`".repeat(longest_backtick_run(text).max(2) + 1);
    let markdown = format!("Writes `{path}`:\n\n{fence}markdown\n{text}{fence}\n");

    Formatter::new()
        .format_terminal(&markdown)
        .unwrap_or(markdown)
}

fn longest_backtick_run(text: &str) -> usize {
    text.split(|c| c != '`').map(str::len).max().unwrap_or(0)
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
