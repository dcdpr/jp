//! The tools this plugin serves, answered when the host's `init` carries a tool
//! call.
//!
//! The host names a tool the way its `source` does, without the `wiki_` prefix
//! the assistant knows it by: `list`, `show`, `create`, and `edit`.
//! Each call runs once and ends with its outcome.
//! The host closed stdin after `init`, so nothing here asks it anything, and a
//! missing argument is an error naming the argument.
//!
//! The tools run what `jp wiki list`, `show`, `add`, and `edit` run, held to
//! the call's access policy file by file through [`Access`].
//! Pages the assistant creates are attributed to the `assistant` option.

use jp_plugin::message::{InitMessage, OutputFormat, ToolAction, ToolCall};
use jp_tool::{AccessPolicy, Action, Context as ToolContext, Outcome};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::{
    Context, Output,
    access::Access,
    cmd,
    edit::{self, Changes},
    is_none,
    options::{Options, Paths},
    page::{PageId, Status},
    parent_arg, show,
};

/// The tools this plugin serves, by the name a `source` gives them, sorted.
///
/// Few, because every tool a model is offered costs context on every turn:
/// `edit` makes every change to a page.
pub(crate) const TOOLS: &[&str] = &["create", "edit", "list", "show"];

/// Answer one tool call.
///
/// Every failure becomes an error outcome, so the assistant reads what went
/// wrong and can correct the call.
pub(crate) fn run(init: &InitMessage, call: &ToolCall, options: &Options) -> Outcome {
    match answer(init, call, options) {
        Ok(text) => text.into(),
        Err(message) => Outcome::Error {
            message,
            trace: vec![],
            transient: true,
        },
    }
}

fn answer(init: &InitMessage, call: &ToolCall, options: &Options) -> Result<String, String> {
    if !TOOLS.contains(&call.name.as_str()) {
        return Err(format!(
            "`jp wiki` serves no tool named `{}`. It serves: {}.",
            call.name,
            TOOLS.join(", ")
        ));
    }
    if call.action == ToolAction::FormatArguments {
        return Err(format!(
            "`{}` has no preview of its own; style its parameters as `function_call` or `json` \
             rather than `tool`.",
            call.name
        ));
    }

    let paths = Paths::new(&init.workspace.root, &init.config, options)?;
    if !paths.wiki.starts_with(&paths.root) {
        return Err(format!(
            "The wiki directory {} is outside the workspace, which is all a tool reaches.",
            paths.wiki
        ));
    }
    let ctx = Context {
        paths,
        config: &init.config,
        options,
        format: OutputFormat::Text,
        access: access(init, call)?,
    };
    let args = Arguments(&call.arguments);

    let output = match call.name.as_str() {
        "list" => list(&ctx, &args),
        "show" => show(&ctx, &args),
        "create" => create(&ctx, &args),
        "edit" => edit(&ctx, &args),
        // `TOOLS` above holds exactly the names matched here.
        other => Err(format!("`jp wiki` serves no tool named `{other}`.")),
    }?;

    Ok(reported(output))
}

/// What the call may read and write: what its access policy grants, inside the
/// workspace.
///
/// A policy the plugin can't read refuses the call rather than reading as
/// unrestricted; a call without one reaches the whole workspace and nothing
/// outside it.
fn access(init: &InitMessage, call: &ToolCall) -> Result<Access, String> {
    let policy = call
        .access
        .clone()
        .map(serde_json::from_value::<AccessPolicy>)
        .transpose()
        .map_err(|error| format!("The access policy for this call can't be read: {error}"))?;

    Ok(Access::Policy(ToolContext {
        root: init.workspace.root.clone(),
        action: Action::Run,
        access: policy,
        workspace_id: init.workspace.id.clone(),
        conversation_id: call.conversation.clone(),
    }))
}

/// A call's arguments, read with errors that name the argument.
struct Arguments<'a>(&'a Map<String, Value>);

impl Arguments<'_> {
    /// An argument the call can't do without.
    ///
    /// Absent and `null` are both missing.
    fn required<T: DeserializeOwned>(&self, key: &str) -> Result<T, String> {
        match self.0.get(key) {
            None | Some(Value::Null) => Err(format!("Missing argument `{key}`.")),
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|error| format!("Argument `{key}` is unusable: {error}.")),
        }
    }

    /// An argument the call can do without; absent and `null` are both `None`.
    fn optional<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, String> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(_) => self.required(key).map(Some),
        }
    }

    /// A flag, `false` when absent.
    fn flag(&self, key: &str) -> Result<bool, String> {
        Ok(self.optional(key)?.unwrap_or(false))
    }

    /// The page `key` names: `W-0a1b2c3`, `W0a1b2c3`, or `0a1b2c3`.
    fn page(&self, key: &str) -> Result<PageId, String> {
        self.required::<String>(key)?
            .parse::<PageId>()
            .map_err(|error| error.to_string())
    }

    /// The `status` argument, read without case.
    fn status(&self) -> Result<Option<Status>, String> {
        self.optional::<String>("status")?
            .map(|status| status.parse())
            .transpose()
    }

    /// A text argument that, given, holds something: an empty one is more
    /// likely a slip than a request.
    fn text(&self, key: &str) -> Result<Option<String>, String> {
        match self.optional::<String>(key)? {
            Some(text) if text.trim().is_empty() => Err(format!("`{key}` must not be empty.")),
            text => Ok(text),
        }
    }
}

/// What a command reported, as the assistant reads it: what it did, what
/// changed, then the warnings about what is left to check.
fn reported(output: Output) -> String {
    let mut text = String::new();
    for note in output.notes {
        push_line(&mut text, &note);
    }
    text.push_str(&output.text);
    for warning in output.warnings {
        push_line(&mut text, &format!("Warning: {warning}"));
    }

    text
}

/// Add `line` to `text` on a line of its own.
fn push_line(text: &mut String, line: &str) {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
}

/// List the pages, the way `jp wiki list` prints them.
///
/// Archived pages are left out unless `status` asks for them.
fn list(ctx: &Context<'_>, args: &Arguments<'_>) -> Result<Output, String> {
    let under = optional_page(args, "under")?;
    let status = args.status()?;
    let labels = args.optional::<Vec<String>>("labels")?.unwrap_or_default();
    let query = args.optional::<String>("query")?;

    let mut output = cmd::list(ctx, &cmd::Filter {
        under,
        status,
        labels: &labels,
        query: query.as_deref(),
        archived: status == Some(Status::Archived),
        all: false,
        full: args.flag("full")?,
    })?;
    if output.text.is_empty() {
        output.text = "No pages match.\n".to_owned();
    }

    Ok(output)
}

/// Show one page, the way `jp wiki show` prints it, fenced as markdown.
///
/// A body or section is the text as written unless `expand_embeds` asks for
/// embeds expanded: the text as written is what `wiki_edit` replaces, so text
/// read this way can be edited and written back without losing an embed.
fn show(ctx: &Context<'_>, args: &Arguments<'_>) -> Result<Output, String> {
    let id = args.page("id")?;
    let body = args.flag("body")?;
    let section = args.optional::<String>("section")?;
    let expand = args.flag("expand_embeds")?;
    if body && section.is_some() {
        return Err("Pass `body` or `section`, not both.".to_owned());
    }

    let mut output = show::show(ctx, id, body, section.as_deref(), !expand)?;
    // The terminal keys syntax highlighting off a leading code fence, so the
    // fence is what gets the page displayed as markdown rather than as a flat
    // wall of text.
    output.text = format!("{}\n", fenced(output.text.trim_end()));

    Ok(output)
}

/// `text` in a markdown code fence longer than any run of backticks inside it,
/// so a fence in a page doesn't close the outer one.
fn fenced(text: &str) -> String {
    let longest = text
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or_default();
    let fence = "`".repeat(longest.max(2) + 1);

    format!("{fence}markdown\n{text}\n{fence}")
}

/// The page argument `name` names, when it is given.
fn optional_page(args: &Arguments<'_>, name: &str) -> Result<Option<PageId>, String> {
    args.optional::<String>(name)?
        .map(|page| page.parse::<PageId>())
        .transpose()
        .map_err(|error| error.to_string())
}

/// Create a page, attributed to the `assistant` option.
///
/// Without a body, the page is a hub: a title and a summary, which the site
/// fills with the pages under it.
fn create(ctx: &Context<'_>, args: &Arguments<'_>) -> Result<Output, String> {
    let title = args.required::<String>("title")?;
    let summary = args.text("summary")?.ok_or("Missing argument `summary`.")?;
    let body = args.optional::<String>("body")?.unwrap_or_default();
    let parent = optional_page(args, "parent")?;
    let labels = args.optional::<Vec<String>>("labels")?.unwrap_or_default();

    cmd::add(ctx, &cmd::Add {
        title: &title,
        status: args.status()?,
        summary: Some(&summary),
        body: Some(&body),
        parent,
        labels: &labels,
        author: &ctx.options.assistant,
    })
}

/// Change any part of a page in one call, leaving the rest as it is.
///
/// `before` or `after` places the page among its siblings once the rest of the
/// edit is kept, which is checked first, against where `parent` moves it, so a
/// placement that can't be made leaves the page unchanged.
fn edit(ctx: &Context<'_>, args: &Arguments<'_>) -> Result<Output, String> {
    let id = args.page("id")?;
    let changes = Changes {
        title: args.optional("title")?,
        summary: args.optional("summary")?,
        status: args.status()?,
        parent: args
            .optional::<String>("parent")?
            .map(|parent| parent_arg(&parent))
            .transpose()?,
        superseded_by: args
            .optional::<String>("superseded_by")?
            .map(|target| (!is_none(&target)).then_some(target)),
        body: args.text("body")?,
        section: args.optional("section")?,
        add_labels: args.optional("add_labels")?.unwrap_or_default(),
        remove_labels: args.optional("remove_labels")?.unwrap_or_default(),
    };

    let rank = match (
        optional_page(args, "before")?,
        optional_page(args, "after")?,
    ) {
        (Some(_), Some(_)) => return Err("Pass `before` or `after`, not both.".to_owned()),
        (Some(other), None) => Some((true, other)),
        (None, Some(other)) => Some((false, other)),
        (None, None) => None,
    };

    let changed = changes != Changes::default();
    if !changed && rank.is_none() {
        return Err(
            "Name what to change: `title`, `summary`, `status`, `parent`, `before`, `after`, \
             `superseded_by`, `body`, `add_labels`, or `remove_labels`."
                .to_owned(),
        );
    }
    if changes.section.is_some() && changes.body.is_none() {
        return Err("`section` names the section `body` replaces; pass the body too.".to_owned());
    }

    if let Some((_, other)) = rank {
        if changes.status == Some(Status::Archived) {
            return Err("An archived page isn't placed among the others.".to_owned());
        }
        cmd::rank_place(&ctx.wiki()?, id, changes.parent, other)?;
    }

    let mut output = if changed {
        edit::edit(ctx, id, &changes)?
    } else {
        Output::default()
    };
    if let Some((before, other)) = rank {
        output.extend(cmd::rank(ctx, id, before, other)?);
    }

    Ok(output)
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
