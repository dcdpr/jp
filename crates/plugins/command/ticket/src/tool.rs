//! The tools this plugin serves, answered when the host's `init` carries a tool
//! call.
//!
//! The host names a tool the way its `source` does, without the `ticket_`
//! prefix the assistant knows it by: `create`, `show`, `list`, and so on.
//! Each call runs once and ends with its outcome.
//! The host closed stdin after `init`, so nothing here asks it anything, and a
//! missing argument is an error naming the argument.
//!
//! Tickets and comments the assistant writes are attributed to the `assistant`
//! option, and comments are rendered with their 1-based positions so a reply
//! can name the comment it answers.
//! `create` and `comment` also answer `format_arguments`, previewing the
//! document they are about to write in the shape it takes on disk.
//!
//! The call's access policy is enforced on the ticket directory before any tool
//! runs: the directory has to be inside the workspace, and the policy has to
//! grant what the tool does there.

use std::{collections::HashMap, fs, io, path::MAIN_SEPARATOR};

use camino::{Utf8Path, Utf8PathBuf};
use chrono::{Local, SecondsFormat, Utc};
use comfort::{
    DEFAULT_MAX_WIDTH,
    format::{FormatOptions, format_markdown_with},
};
use jp_md::format::Formatter;
use jp_plugin::message::{InitMessage, ToolAction, ToolCall};
use jp_tool::{AccessPolicy, Action, Capability, Context, FsAccessError, Outcome, Question};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use ticket::{
    Comment, Labels, NewTicket, Selector, Status, Ticket, TicketId, Vocabulary,
    board::{self, Order},
    checklist::{self, Marked},
    parse, render, store,
};

use crate::{label, options::Options};

type Error = Box<dyn std::error::Error + Send + Sync>;

/// What a tool answers: its outcome, or a failure that becomes an error outcome
/// on the way out.
type Answer = Result<Outcome, Error>;

/// The tools this plugin serves, by the name a `source` gives them.
///
/// Few, because every tool a model is offered costs context on every turn:
/// `edit` changes any part of a ticket in one call.
pub(crate) const TOOLS: &[&str] = &["comment", "create", "edit", "list", "show"];

/// The longest title `create` accepts, in characters, unless the tool's
/// `options.max_title_length` overrides it.
///
/// Tied to the slug budget so the filename carries the whole title: a slug is
/// never longer than the title it came from, since each character contributes
/// at most one ASCII byte.
pub(crate) const DEFAULT_MAX_TITLE_LENGTH: usize = store::MAX_SLUG_SIZE;

/// How many tickets a page of `list` holds, unless the tool's
/// `options.page_size` overrides it.
pub(crate) const DEFAULT_PAGE_SIZE: usize = 50;

/// The question `create` puts back when a title doesn't fit.
///
/// The tool's definition routes it to the assistant, and the answer must be
/// read under this same id.
pub(crate) const SHORTER_TITLE: &str = "shorter_title";

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
    let Some(capabilities) = capabilities(call) else {
        return refuse(format!(
            "`jp ticket` serves no tool named `{}`. It serves: {}.",
            call.name,
            TOOLS.join(", ")
        ));
    };

    let preview = call.action == ToolAction::FormatArguments;
    if preview && !matches!(call.name.as_str(), "create" | "comment") {
        return refuse(format!(
            "`{}` has no preview of its own; style its parameters as `function_call` or `json` \
             rather than `tool`.",
            call.name
        ));
    }

    let tickets = match Tickets::open(init, call, options, capabilities) {
        Ok(tickets) => tickets,
        Err(refusal) => return refuse(refusal),
    };
    let args = Arguments(&call.arguments);

    match call.name.as_str() {
        "create" => create(&tickets, &args, call, preview),
        "comment" => comment(&tickets, &args, preview),
        "edit" => edit(&tickets, &args),
        "show" => show(&tickets, &args),
        "list" => list(&tickets, &args, call),
        // `capabilities` above answers for exactly the names in `TOOLS`.
        other => refuse(format!("`jp ticket` serves no tool named `{other}`.")),
    }
}

/// What a call does in the ticket directory, in the access policy's terms.
///
/// `None` for a name this plugin serves no tool under.
fn capabilities(call: &ToolCall) -> Option<&'static [Capability]> {
    let retitles = call
        .arguments
        .get("title")
        .is_some_and(|title| !title.is_null());

    Some(match call.name.as_str() {
        "show" | "list" => &[Capability::Read],
        "create" => &[Capability::Read, Capability::Create],
        // A new title moves the file: one name goes and another is created.
        "edit" if retitles => &[
            Capability::Read,
            Capability::Update,
            Capability::Create,
            Capability::Delete,
        ],
        "comment" | "edit" => &[Capability::Read, Capability::Update],
        _ => return None,
    })
}

/// Where a call reads and writes tickets, once its access policy allowed it.
struct Tickets<'a> {
    /// The workspace root, which paths in results are relative to.
    root: &'a Utf8Path,

    /// The ticket directory.
    dir: Utf8PathBuf,

    /// Who what the call files is attributed to.
    author: &'a str,

    /// The plugin's settings, which hold the board's label vocabulary.
    options: &'a Options,
}

impl<'a> Tickets<'a> {
    /// Check the call may do what `capabilities` names to the ticket directory.
    ///
    /// A policy the plugin can't read refuses the call rather than reading as
    /// unrestricted, and a directory outside the workspace is refused whatever
    /// the policy says.
    /// The error is the refusal, for the assistant.
    fn open(
        init: &'a InitMessage,
        call: &ToolCall,
        options: &'a Options,
        capabilities: &[Capability],
    ) -> Result<Self, String> {
        let root = init.workspace.root.as_path();
        let relative = workspace_relative(root, &options.dir)?;

        let access = call
            .access
            .clone()
            .map(serde_json::from_value::<AccessPolicy>)
            .transpose()
            .map_err(|error| format!("The access policy for this call can't be read: {error}"))?;

        let ctx = Context {
            root: root.to_owned(),
            action: match call.action {
                ToolAction::Run => Action::Run,
                ToolAction::FormatArguments => Action::FormatArguments,
            },
            access,
            workspace_id: init.workspace.id.clone(),
            conversation_id: call.conversation.clone(),
        };

        for capability in capabilities {
            check(&ctx, &relative, *capability).map_err(|error| {
                format!("`{}` can't reach the ticket directory: {error}", call.name)
            })?;
        }

        Ok(Self {
            root,
            dir: root.join(relative),
            author: &options.assistant,
            options,
        })
    }

    /// A path the model can hand straight to `fs_read_file`.
    ///
    /// Separators are always `/`, on every platform: the path travels through
    /// tool results into the conversation, and `fs_read_file` takes forward
    /// slashes everywhere.
    fn relative(&self, path: &Utf8Path) -> String {
        path.strip_prefix(self.root)
            .unwrap_or(path)
            .as_str()
            .replace(MAIN_SEPARATOR, "/")
    }
}

/// The ticket directory as the access policy reads paths: relative to the
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
                "The ticket directory {dir} is outside the workspace, which is all a tool reaches."
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

    /// The ticket the `id` argument names.
    ///
    /// The error is the refusal, for the assistant.
    fn id(&self) -> Result<Result<TicketId, String>, Error> {
        Ok(id_arg(&self.required::<Value>("id")?))
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

fn create(tickets: &Tickets<'_>, args: &Arguments<'_>, call: &ToolCall, preview: bool) -> Answer {
    let title = args.required::<String>("title")?;
    let body = args.optional::<String>("body")?;
    let parent = match args.optional::<Value>("parent")?.as_ref().map(id_arg) {
        Some(Ok(parent)) => Some(parent),
        Some(Err(refusal)) => return refuse(format!("`parent`: {refusal}")),
        None => None,
    };

    // Checked before the action split so a preview fails too: an unattended
    // formatter that errors fails the call ahead of the approval prompt, which
    // tells the assistant to fix the arguments rather than asking the user
    // about a call that cannot land.
    if title.trim().is_empty() {
        return refuse("`title` must not be empty.");
    }
    let vocabulary = tickets.options.vocabulary()?;
    let labels = match resolve_labels(
        &vocabulary,
        args.optional::<Vec<String>>("labels")?.as_deref(),
    ) {
        Ok(labels) => labels,
        Err(refusal) => return refuse(refusal),
    };
    match store::check_unique(&tickets.dir, &vocabulary, &labels, None) {
        Ok(()) => {}
        Err(taken @ store::Error::Taken(_)) => return refuse(taken.to_string()),
        Err(other) => return Err(other.into()),
    }
    if parent.is_some() {
        match store::check_relations(&tickets.dir, None, Status::Todo, parent) {
            Ok(()) => {}
            Err(refused @ store::Error::Parent(_)) => return refuse(refused.to_string()),
            Err(other) => return Err(other.into()),
        }
    }

    let limit = option_or(&call.options, "max_title_length", DEFAULT_MAX_TITLE_LENGTH);
    let title = match fitting_title(&title, &call.answers, limit) {
        Ok(title) => title,
        Err(outcome) => return outcome,
    };

    let date = Local::now().format("%Y-%m-%d").to_string();
    let new = NewTicket {
        title: &title,
        authors: tickets.author,
        date: &date,
        labels: &labels,
        description: body.as_deref().unwrap_or_default(),
        parent,
    };

    if preview {
        return Ok(preview_create(&new).into());
    }

    let (id, path) = store::create(&tickets.dir, &vocabulary, &new)?;
    reflow(&path)?;

    Ok(match parent {
        Some(parent) => format!("Created {id} under {parent} at {}", tickets.relative(&path)),
        None => format!("Created {id} at {}", tickets.relative(&path)),
    }
    .into())
}

/// Settle on a title of at most `limit` characters.
///
/// A title over the limit is put back as a [`SHORTER_TITLE`] question, so the
/// ticket can be retitled without the body being sent a second time.
/// The question's default is the title as given, so a prompt opens with it
/// ready to edit, and its preamble names that title, so an assistant asked to
/// shorten it sees the one being filed rather than the one it first sent.
/// The answer replaces the title outright; one that is itself unusable ends the
/// call, because asking again under the same id would put the tool straight
/// back where it started.
///
/// The error variant is the outcome to return in place of the caller's work.
/// Both actions go through here: a preview drawn before the title is settled
/// would show a heading and a filename the ticket won't carry, so the question
/// is put back there too and the preview is drawn once the answer is in.
fn fitting_title(
    title: &str,
    answers: &Map<String, Value>,
    limit: usize,
) -> Result<String, Answer> {
    let Some(answer) = answers.get(SHORTER_TITLE) else {
        let title = title.trim();
        let count = title.chars().count();
        if count <= limit {
            return Ok(title.to_owned());
        }

        let question = match Question::text(
            SHORTER_TITLE,
            format!("Shorter title, at most {limit} characters?"),
        ) {
            Ok(question) => question
                .with_preamble(format!("The title is {count} characters: {title}"))
                .with_default(title),
            Err(err) => return Err(Err(err.into())),
        };

        return Err(Ok(question.into()));
    };

    let retitled = answer.as_str().unwrap_or_default().trim();
    if retitled.is_empty() {
        return Err(refuse("The shorter title was empty."));
    }

    let count = retitled.chars().count();
    if count > limit {
        return Err(refuse(format!(
            "The shorter title is {count} characters, and a ticket takes at most {limit}: \
             {retitled}"
        )));
    }

    Ok(retitled.to_owned())
}

/// Render the ticket file `create` is about to write.
///
/// The id is left out because the file doesn't carry one: it is drawn when the
/// ticket is claimed, and the result names it.
fn preview_create(new: &NewTicket<'_>) -> String {
    preview(&render::ticket(new))
}

/// Read a tool option, falling back to `default` when it is missing or of
/// another type.
fn option_or<T: DeserializeOwned>(options: &Map<String, Value>, key: &str, default: T) -> T {
    options
        .get(key)
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or(default)
}

/// Check labels for a ticket being filed against the board's vocabulary.
///
/// A call that names no labels is checked too, since the board can require
/// some.
/// The error is the caller's, a refusal naming what the board takes.
fn resolve_labels(vocabulary: &Vocabulary, requested: Option<&[String]>) -> Result<Labels, String> {
    vocabulary
        .resolve(requested.unwrap_or_default())
        .map_err(|refusal| refusal.to_string())
}

fn comment(tickets: &Tickets<'_>, args: &Arguments<'_>, preview: bool) -> Answer {
    let id = match args.id()? {
        Ok(id) => id,
        Err(refusal) => return refuse(refusal),
    };
    let re = args.optional::<usize>("re")?;
    let body = args.required::<String>("body")?;

    if body.trim().is_empty() {
        return refuse("`body` must not be empty.");
    }

    let date = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    if preview {
        return preview_comment(tickets, id, re, &body, &date);
    }

    match store::append_comment(&tickets.dir, id, tickets.author, &date, re, body.trim()) {
        Ok(position) => {
            reflow(&store::locate_ticket(&tickets.dir, id)?)?;
            Ok(format!("Added {id}#{position}").into())
        }
        Err(store::Error::NoSuchTicket(_) | store::Error::NoSuchComment { .. }) => refuse(format!(
            "No {id}, or no comment #{} on it.",
            re.unwrap_or(0)
        )),
        Err(other) => Err(other.into()),
    }
}

/// Render the comment block `comment` is about to append, under the heading of
/// the ticket it lands on.
///
/// Fails when the ticket or the reply target isn't there, the same two
/// conditions the append itself rejects, so a call that cannot land is answered
/// before it is put to the user.
fn preview_comment(
    tickets: &Tickets<'_>,
    id: TicketId,
    re: Option<usize>,
    body: &str,
    date: &str,
) -> Answer {
    let path = match store::locate_ticket(&tickets.dir, id) {
        Ok(path) => path,
        Err(store::Error::NoSuchTicket(_)) => return refuse(format!("No {id}.")),
        Err(other) => return Err(other.into()),
    };
    let document = fs::read_to_string(path)?;

    // The count comes from the same tolerant reader the append uses, so the
    // two agree on a file with a hand-mangled header.
    let count = parse::comment_count(&document);
    if let Some(position) = re
        && (position == 0 || position > count)
    {
        return refuse(format!("No comment #{position} on {id}."));
    }

    // The heading names the ticket, which its own file doesn't: the id is what
    // makes the preview readable next to the call that produced it.
    let heading = match parse::title(&document) {
        Some(title) => format!("# {id}: {title}"),
        None => format!("# {id}"),
    };

    let comment = Comment {
        from: tickets.author.to_owned(),
        date: date.to_owned(),
        re: re.map(|position| format!("#{position}")),
        body: body.to_owned(),
    };

    Ok(preview(&format!("{heading}\n\n{}", render::comment(&comment))).into())
}

/// Change any of a ticket's title, description, status, labels, checklist, and
/// parent in one call, leaving the rest as it is.
///
/// Labels are added and removed rather than replaced, so a call can't drop one
/// it doesn't name: `remove_labels` first, where a bare key takes every label
/// under it, then `add_labels`.
/// Checklist items are named by their text in `check` and `uncheck`, and only
/// their checkboxes change.
/// `parent` takes a ticket id, or `none` to take the parent away.
///
/// Every argument is checked before anything is written, the checklist items
/// named and where the status and the parent leave the ticket included.
/// The labels, which the vocabulary can still refuse, are written first, so a
/// refused call changes nothing; the status and the parent together, judged by
/// where they leave the ticket; the title last, since it moves the file.
/// Closing a ticket whose checklist has unchecked items is allowed, and warned
/// about.
fn edit(tickets: &Tickets<'_>, args: &Arguments<'_>) -> Answer {
    let id = match args.id()? {
        Ok(id) => id,
        Err(refusal) => return refuse(refusal),
    };
    let title = args.optional::<String>("title")?;
    let description = args.optional::<String>("description")?;
    let status = match args
        .optional::<String>("status")?
        .map(|status| status.parse::<Status>())
    {
        Some(Err(_)) => return refuse("`status` must be one of: todo, in progress, done."),
        Some(Ok(status)) => Some(status),
        None => None,
    };
    let add = args
        .optional::<Vec<String>>("add_labels")?
        .unwrap_or_default();
    let remove = args
        .optional::<Vec<String>>("remove_labels")?
        .unwrap_or_default();
    let relabels = !add.is_empty() || !remove.is_empty();
    let check = args.optional::<Vec<String>>("check")?.unwrap_or_default();
    let uncheck = args.optional::<Vec<String>>("uncheck")?.unwrap_or_default();
    let marks = !check.is_empty() || !uncheck.is_empty();
    let parent = match parent_change(args)? {
        Ok(parent) => parent,
        Err(refusal) => return refuse(refusal),
    };

    if title.is_none()
        && description.is_none()
        && status.is_none()
        && parent.is_none()
        && !relabels
        && !marks
    {
        return refuse(
            "Name what to change: `title`, `description`, `status`, `parent`, `check`, `uncheck`, \
             `add_labels`, or `remove_labels`.",
        );
    }
    if let Err(error) = Labels::from_tokens(&remove) {
        return refuse(format!("`remove_labels`: {error}."));
    }
    if title
        .as_deref()
        .is_some_and(|title| title.trim().is_empty())
    {
        return refuse("`title` must not be empty.");
    }
    // An empty description is more likely a slip than a request, and the write
    // replaces the old one outright.
    if description
        .as_deref()
        .is_some_and(|description| description.trim().is_empty())
    {
        return refuse("`description` must not be empty.");
    }

    let path = match store::locate_ticket(&tickets.dir, id) {
        Ok(path) => path,
        Err(store::Error::NoSuchTicket(_)) => return refuse(format!("No {id}.")),
        Err(other) => return Err(other.into()),
    };
    if status.is_some() || parent.is_some() {
        match store::check_relate(&tickets.dir, id, status, parent) {
            Ok(()) => {}
            Err(refused @ store::Error::Parent(_)) => return refuse(refused.to_string()),
            Err(other) => return Err(other.into()),
        }
    }

    let closes = status == Some(Status::Done);
    let content = match settle_content(&path, description.as_deref(), &check, &uncheck, closes)? {
        Ok(content) => content,
        Err(refusal) => return refuse(refusal),
    };

    let mut changed = vec![];
    if relabels {
        match edit_labels(tickets, id, &add, &remove)? {
            Ok(what) => changed.push(format!("labels: {what}")),
            Err(refusal) => return refuse(refusal),
        }
    }
    if status.is_some() || parent.is_some() {
        let related = store::relate(&tickets.dir, id, status, parent)?;
        changed.extend(describe_related(&related, status, parent));
    }
    changed.extend(write_content(
        tickets,
        id,
        title.as_deref(),
        &content,
        description.is_some(),
    )?);
    if let Some(warning) = content.left.as_deref().and_then(unchecked_on_close) {
        changed.push(format!("warning: {warning}"));
    }

    let mut text = format!("Edited {id}:\n");
    for change in changed {
        text.push_str(&format!("- {change}\n"));
    }

    Ok(text.into())
}

/// What an `edit` call does to a ticket's description, settled before anything
/// is written.
struct Content {
    /// The checklist items `check` and `uncheck` named, and the description
    /// they leave.
    marked: Option<Marked>,

    /// The description to write, or `None` when it stays as it is, which it
    /// does when every item named is in place already.
    rewritten: Option<String>,

    /// The description the call leaves the ticket with, when the call closes it
    /// and the checklist has to be judged.
    left: Option<String>,
}

/// Settle what `edit` does to the description of the ticket at `path`: write
/// the `description` it was given, or check and uncheck the items `check` and
/// `uncheck` name.
///
/// The ticket is read only when its checklist is needed: to mark items, or
/// because the call `closes` it.
/// The inner error is the refusal, for the assistant: a description and items
/// to mark both given, or an item named wrongly.
fn settle_content(
    path: &Utf8Path,
    description: Option<&str>,
    check: &[String],
    uncheck: &[String],
    closes: bool,
) -> Result<Result<Content, String>, Error> {
    let marks = !check.is_empty() || !uncheck.is_empty();
    if description.is_some() && marks {
        return Ok(Err("Pass `description` or `check` and `uncheck`, not \
                       both: check the items in the new description itself."
            .to_owned()));
    }
    let current = if marks || closes {
        Some(parse::document(&fs::read_to_string(path)?)?)
    } else {
        None
    };
    let marked = match current.as_ref().filter(|_| marks) {
        Some(ticket) => match checklist::mark(&ticket.description, check, uncheck) {
            Ok(marked) => Some(marked),
            Err(refused) => return Ok(Err(refused.to_string())),
        },
        None => None,
    };

    // Nothing is written when every item named is in place already, so a
    // hand-edited file isn't reflowed for a call that changes nothing in it.
    let rewritten = description.map(ToOwned::to_owned).or_else(|| {
        marked
            .as_ref()
            .filter(|marked| marked.changes.iter().any(|change| change.changed))
            .map(|marked| marked.description.clone())
    });
    let left = if closes {
        description
            .map(ToOwned::to_owned)
            .or_else(|| marked.as_ref().map(|marked| marked.description.clone()))
            .or_else(|| current.map(|ticket| ticket.description))
    } else {
        None
    };

    Ok(Ok(Content {
        marked,
        rewritten,
        left,
    }))
}

/// Write the title and the description `edit` settled on, returning what
/// changed, in words.
///
/// `replaced` is whether the call gave a description of its own, rather than
/// checklist items to mark.
fn write_content(
    tickets: &Tickets<'_>,
    id: TicketId,
    title: Option<&str>,
    content: &Content,
    replaced: bool,
) -> Result<Vec<String>, Error> {
    let edited = if title.is_some() || content.rewritten.is_some() {
        let edited = store::edit(
            &tickets.dir,
            id,
            title.map(str::trim),
            content.rewritten.as_deref(),
        )?;
        if content.rewritten.is_some() {
            reflow(&edited.to)?;
        }
        Some(edited)
    } else {
        None
    };

    let mut changed = vec![];
    if replaced {
        changed.push("description replaced".to_owned());
    }
    if let Some(marked) = &content.marked {
        changed.extend(describe_marked(marked));
    }
    if let (Some(_), Some(store::Edited { from, to })) = (title, &edited) {
        changed.push(if from == to {
            format!("title changed, still at {}", tickets.relative(to))
        } else {
            format!(
                "title changed, moved {} -> {}",
                tickets.relative(from),
                tickets.relative(to)
            )
        });
    }

    Ok(changed)
}

/// What `edit` says about the checklist items it was asked to check and
/// uncheck: a line per item, then the checklist's progress.
fn describe_marked(marked: &Marked) -> Vec<String> {
    let mut lines: Vec<String> = marked
        .changes
        .iter()
        .map(|change| {
            let state = if change.checked {
                "checked"
            } else {
                "unchecked"
            };
            let already = if change.changed { "" } else { "already " };

            format!("{already}{state}: {}", change.text)
        })
        .collect();
    lines.push(format!("checklist: {}", marked.progress));

    lines
}

/// What closing a ticket whose description is `description` leaves unchecked,
/// as a warning, or `None` when every item is checked.
///
/// Closing isn't refused: the items are the ticket's own, and whoever closes it
/// decides what is left of them.
pub(crate) fn unchecked_on_close(description: &str) -> Option<String> {
    let items = checklist::items(description);
    let open: Vec<String> = items
        .iter()
        .filter(|item| !item.checked)
        .map(|item| format!("{:?}", item.text))
        .collect();
    if open.is_empty() {
        return None;
    }

    Some(format!(
        "closed with {} of {} checklist items unchecked: {}. Check the ones that are done, and \
         remove the rest with a comment saying why.",
        open.len(),
        items.len(),
        open.join(", ")
    ))
}

/// Read `edit`'s `parent` argument: `None` when it is absent, `Some(None)` for
/// `none`.
///
/// The inner error is the refusal, for the assistant.
fn parent_change(args: &Arguments<'_>) -> Result<Result<Option<Option<TicketId>>, String>, Error> {
    Ok(match args.optional::<Value>("parent")? {
        None => Ok(None),
        Some(Value::String(none)) if none.trim().eq_ignore_ascii_case("none") => Ok(Some(None)),
        Some(value) => id_arg(&value)
            .map(|parent| Some(Some(parent)))
            .map_err(|refusal| format!("`parent`: {refusal}")),
    })
}

/// What `edit` says about the status and parent it asked for, given what
/// `relate` found before writing them.
fn describe_related(
    related: &store::Related,
    status: Option<Status>,
    parent: Option<Option<TicketId>>,
) -> Vec<String> {
    let name = |parent: Option<TicketId>| parent.map_or("none".to_owned(), |id| id.to_string());
    let mut changed = vec![];

    if let Some(status) = status {
        changed.push(if related.previous_status == status {
            format!("status: already {status}")
        } else {
            format!("status: {} -> {status}", related.previous_status)
        });
    }
    if let Some(parent) = parent {
        changed.push(if related.previous_parent == parent {
            format!("parent: already {}", name(parent))
        } else {
            format!(
                "parent: {} -> {}",
                name(related.previous_parent),
                name(parent)
            )
        });
    }

    changed
}

/// Apply `edit`'s label changes to a ticket: `remove`, then `add`.
///
/// Returns what changed, in words; the inner error is the board's refusal, for
/// the assistant.
fn edit_labels(
    tickets: &Tickets<'_>,
    id: TicketId,
    add: &[String],
    remove: &[String],
) -> Result<Result<String, String>, Error> {
    let vocabulary = tickets.options.vocabulary()?;
    let relabeled = store::relabel(&tickets.dir, &vocabulary, &[id], |current| {
        let mut tokens = label::remove(current, remove);
        tokens.extend(add.iter().cloned());
        tokens
    });

    match relabeled {
        Ok(relabeled) => Ok(Ok(relabeled.first().map_or_else(String::new, |ticket| {
            label::describe(&ticket.before, &ticket.after, add, remove)
        }))),
        Err(refusal @ (store::Error::Rejected(_) | store::Error::Taken(_))) => {
            Ok(Err(refusal.to_string()))
        }
        Err(other) => Err(other.into()),
    }
}

/// Read one ticket: its metadata, its parent and children, and the description,
/// the comments, and the parent's description when the call asks for them.
fn show(tickets: &Tickets<'_>, args: &Arguments<'_>) -> Answer {
    let id = match args.id()? {
        Ok(id) => id,
        Err(refusal) => return refuse(refusal),
    };
    let parts = Parts {
        description: args.optional("description")?.unwrap_or(false),
        comments: args.optional("comments")?.unwrap_or(false),
        parent_description: args.optional("parent_description")?.unwrap_or(false),
    };

    let entries = store::list(&tickets.dir)?;
    let Some(entry) = entries.iter().find(|entry| {
        entry
            .path
            .file_name()
            .is_some_and(|name| name.starts_with(&id.file_prefix()))
    }) else {
        return refuse(format!("No {id}."));
    };

    match &entry.ticket {
        Ok(ticket) => {
            // An order file that can't be read costs the children their order,
            // as it costs `list` its columns'.
            let order = board::read(&tickets.dir).unwrap_or_default();
            let family = Family {
                entries: &entries,
                children: children_of(&entries, entry.id, &order),
            };
            let rendered = render_ticket(
                entry.id,
                ticket,
                &tickets.relative(&entry.path),
                &family,
                parts,
            );
            // The terminal keys syntax highlighting off a leading code fence,
            // so the fence is what gets a ticket displayed as markdown rather
            // than as a flat wall of text.
            Ok(format!("```markdown\n{}\n```", rendered.trim_end()).into())
        }
        Err(problem) => refuse(format!("{id} is not a well-formed ticket: {problem}")),
    }
}

/// List the board, filtered by whatever the caller named, one page at a time.
///
/// Tickets come in board order: Todo, In Progress, then Done, each column top
/// first, so the first Todo line is the next thing to work on.
/// Labels are matched as written on the ticket rather than through the
/// vocabulary, so a ticket carrying a label the board has since dropped can
/// still be found.
fn list(tickets: &Tickets<'_>, args: &Arguments<'_>, call: &ToolCall) -> Answer {
    let status = match args
        .optional::<String>("status")?
        .map(|s| s.parse::<Status>())
    {
        Some(Err(_)) => return refuse("`status` must be one of: todo, in progress, done."),
        Some(Ok(status)) => Some(status),
        None => None,
    };
    let labels = args.optional::<Vec<String>>("labels")?.unwrap_or_default();
    let query = args
        .optional::<String>("query")?
        .map(|query| query.trim().to_owned())
        .filter(|query| !query.is_empty());
    let under = match args.optional::<Value>("under")?.as_ref().map(id_arg) {
        Some(Ok(under)) => Some(under),
        Some(Err(refusal)) => return refuse(format!("`under`: {refusal}")),
        None => None,
    };
    let page = args.optional::<usize>("page")?.unwrap_or(1);
    if page == 0 {
        return refuse("`page` counts from 1.");
    }
    let page_size = option_or(&call.options, "page_size", DEFAULT_PAGE_SIZE).max(1);

    let entries = store::list(&tickets.dir)?;

    let mut found = vec![];
    let mut unreadable = vec![];
    for entry in &entries {
        match &entry.ticket {
            Ok(ticket) => found.push((entry.id, ticket)),
            Err(problem) => {
                unreadable.push(format!("{}: {problem}", tickets.relative(&entry.path)));
            }
        }
    }
    let selectors = match Selector::parse_all(&labels) {
        Ok(selectors) => selectors,
        Err(problem) => return refuse(problem.to_string()),
    };
    found.retain(|(_, ticket)| {
        status.is_none_or(|status| status == ticket.metadata.status)
            && ticket.metadata.labels.matches(&selectors)
            && query.as_deref().is_none_or(|query| ticket.mentions(query))
            && under.is_none_or(|under| ticket.metadata.parent == Some(under))
    });

    // An order file that can't be read costs the order, not the listing: the
    // columns fall back to id order, and the reader is told why.
    let order = match board::read(&tickets.dir) {
        Ok(order) => order,
        Err(problem) => {
            let file = tickets.relative(&tickets.dir.join(board::BOARD_FILE));
            let reason = match problem {
                store::Error::Board { reason, .. } => reason,
                other => other.to_string(),
            };
            unreadable.push(format!("{file}: {reason} (columns are listed by id)"));
            Order::default()
        }
    };

    let laid_out = in_board_order(&found, &order);
    let count = laid_out.len();
    let pages = count.div_ceil(page_size).max(1);
    if page > pages {
        return refuse(format!("Page {page} is past the last page, {pages}."));
    }

    let start = (page - 1) * page_size;
    let shown = &laid_out[start..count.min(start + page_size)];
    let footer = (pages > 1).then(|| {
        let mut footer = format!("Page {page} of {pages} ({count} tickets).");
        if page < pages {
            footer.push_str(&format!(" Pass `page: {}` for more.", page + 1));
        }
        footer
    });

    Ok(render_list(shown, footer.as_deref(), &unreadable).into())
}

/// The readable tickets among `entries` filed under `id`, in board order.
pub(crate) fn children_of<'a>(
    entries: &'a [store::Entry],
    id: TicketId,
    order: &Order,
) -> Vec<(TicketId, &'a Ticket)> {
    let children: Vec<(TicketId, &Ticket)> = entries
        .iter()
        .filter_map(|entry| Some((entry.id, entry.ticket.as_ref().ok()?)))
        .filter(|(child, ticket)| *child != id && ticket.metadata.parent == Some(id))
        .collect();

    in_board_order(&children, order)
}

/// A parent as a child's metadata shows it: its id, then its status and title
/// when it can be read.
pub(crate) fn describe_parent(entries: &[store::Entry], parent: TicketId) -> String {
    let found = entries
        .iter()
        .find(|entry| entry.id == parent)
        .and_then(|entry| entry.ticket.as_ref().ok());

    match found {
        Some(ticket) => format!("{parent} ({}) {}", ticket.metadata.status, ticket.title),
        None => format!("{parent} (not found)"),
    }
}

/// The heading above a parent's children, counting how many are Done.
pub(crate) fn children_heading(children: &[(TicketId, &Ticket)]) -> String {
    let done = children
        .iter()
        .filter(|(_, ticket)| ticket.metadata.status == Status::Done)
        .count();

    format!("## Children ({done} of {} done)", children.len())
}

/// `found` laid out the way the board shows it: Todo, In Progress, then Done,
/// each column in `order`.
fn in_board_order<'a>(
    found: &[(TicketId, &'a Ticket)],
    order: &Order,
) -> Vec<(TicketId, &'a Ticket)> {
    let by_id: HashMap<TicketId, &Ticket> = found.iter().copied().collect();

    let mut laid_out = Vec::with_capacity(found.len());
    for status in [Status::Todo, Status::InProgress, Status::Done] {
        let column: Vec<TicketId> = found
            .iter()
            .filter(|(_, ticket)| ticket.metadata.status == status)
            .map(|(id, _)| *id)
            .collect();

        laid_out.extend(
            order
                .arrange(status, &column)
                .into_iter()
                .filter_map(|id| by_id.get(&id).map(|ticket| (id, *ticket))),
        );
    }

    laid_out
}

/// Lay out a markdown file the way `comfort` does.
///
/// A body arrives as the model wrote it, one long line per paragraph, and the
/// repository's markdown carries semantic line breaks.
/// The options mirror the `fmt-markdown-ci` recipe in JP's justfile, so a
/// ticket written here is one that recipe accepts as it stands.
fn reflow(path: &Utf8Path) -> io::Result<()> {
    let source = fs::read_to_string(path)?;
    let formatted = format_markdown_with(&source, &FormatOptions {
        max_width: DEFAULT_MAX_WIDTH,
        canonical: true,
        reference_links: true,
        prune_reference_links: true,
    });

    if formatted == source {
        return Ok(());
    }

    fs::write(path, formatted)
}

/// Prefix every line of `document` with a markdown blockquote marker.
///
/// Empty lines get a bare `>` so the rail stays unbroken without trailing
/// whitespace.
fn quote(document: &str) -> String {
    let mut quoted = String::with_capacity(document.len() * 2);
    for line in document.lines() {
        quoted.push('>');
        if !line.is_empty() {
            quoted.push(' ');
            quoted.push_str(line);
        }
        quoted.push('\n');
    }

    quoted
}

/// Style a document for the terminal as a tool-call preview.
///
/// The document is quoted first, so the transcript carries a marker down the
/// whole preview and the reader can see where the tool output ends and the
/// conversation resumes.
/// Falls back to the unstyled source if the markdown can't be formatted.
fn preview(document: &str) -> String {
    let quoted = quote(document);

    Formatter::new().format_terminal(&quoted).unwrap_or(quoted)
}

/// Render one page of the board as one line per ticket, then `footer`, which
/// says where the page sits when there are more.
fn render_list(
    tickets: &[(TicketId, &Ticket)],
    footer: Option<&str>,
    unreadable: &[String],
) -> String {
    let mut out = String::new();

    if tickets.is_empty() {
        out.push_str("No tickets match.\n");
    }
    for (id, ticket) in tickets {
        let id = id.to_string();
        let status = ticket.metadata.status.to_string();
        let comments = match ticket.comments.len() {
            0 => String::new(),
            1 => " (1 comment)".to_owned(),
            count => format!(" ({count} comments)"),
        };
        let labels = if ticket.metadata.labels.is_empty() {
            String::new()
        } else {
            format!(" [{}]", ticket.metadata.labels)
        };
        let checklist = ticket
            .checklist()
            .map_or_else(String::new, |progress| format!(" ({progress})"));

        out.push_str(&format!(
            "{id:<9} {status:<12} {}{labels}{checklist}{comments}\n",
            ticket.title
        ));
    }

    if let Some(footer) = footer {
        out.push_str(&format!("\n{footer}\n"));
    }

    if !unreadable.is_empty() {
        out.push_str("\nCould not read:\n");
        for problem in unreadable {
            out.push_str(&format!("- {problem}\n"));
        }
    }

    out
}

/// What `show` prints besides a ticket's metadata.
#[derive(Debug, Clone, Copy, Default)]
struct Parts {
    description: bool,
    comments: bool,
    parent_description: bool,
}

/// The tickets a ticket being shown is related to.
struct Family<'a> {
    /// Every ticket, where the parent is looked up.
    entries: &'a [store::Entry],

    /// The tickets filed under the one shown, in board order.
    children: Vec<(TicketId, &'a Ticket)>,
}

impl Family<'_> {
    /// The parent of `ticket`, when it names one that can be read.
    fn parent_of(&self, ticket: &Ticket) -> Option<(TicketId, &Ticket)> {
        let parent = ticket.metadata.parent?;
        let entry = self.entries.iter().find(|entry| entry.id == parent)?;

        Some((parent, entry.ticket.as_ref().ok()?))
    }
}

/// Render one ticket's metadata, its parent and children, and the parts `parts`
/// asks for.
///
/// Comments are numbered so a reply can name its target.
/// The parent's description is the brief its children share, so it is offered
/// on every child rather than copied into each.
/// A part left out that the ticket has is named at the end, with the argument
/// that shows it, so the reader knows there is more without asking.
fn render_ticket(
    id: TicketId,
    ticket: &Ticket,
    path: &str,
    family: &Family<'_>,
    parts: Parts,
) -> String {
    let metadata = &ticket.metadata;
    // The rendered view names the ticket even though the file doesn't: the id
    // is what a reply or a later reference has to quote.
    let mut out = format!("# {id}: {}\n\n", ticket.title);

    out.push_str(&format!("- **Path**: {path}\n"));
    out.push_str(&format!("- **Status**: {}\n", metadata.status));
    for token in metadata.labels.to_tokens() {
        out.push_str(&format!("- **Label**: {token}\n"));
    }
    out.push_str(&format!("- **Authors**: {}\n", metadata.authors));
    out.push_str(&format!("- **Date**: {}\n", metadata.date));
    for (label, value) in [
        ("Promoted to", &metadata.promoted_to),
        ("GitHub", &metadata.github),
    ] {
        if let Some(value) = value {
            out.push_str(&format!("- **{label}**: {value}\n"));
        }
    }
    if let Some(parent) = metadata.parent {
        out.push_str(&format!(
            "- **Parent**: {}\n",
            describe_parent(family.entries, parent)
        ));
    }
    if let Some(progress) = ticket.checklist() {
        out.push_str(&format!("- **Checklist**: {progress}\n"));
    }

    if parts.description && !ticket.description.is_empty() {
        out.push_str(&format!("\n{}\n", ticket.description));
    }

    let parent = family.parent_of(ticket);
    if parts.parent_description
        && let Some((parent_id, parent)) = parent
        && !parent.description.is_empty()
    {
        out.push_str(&format!(
            "\n## Parent {parent_id}: {}\n\n{}\n",
            parent.title, parent.description
        ));
    }

    if !family.children.is_empty() {
        out.push_str(&format!("\n{}\n\n", children_heading(&family.children)));
        for (child, child_ticket) in &family.children {
            out.push_str(&format!(
                "- {child} ({}) {}\n",
                child_ticket.metadata.status, child_ticket.title
            ));
        }
    }

    if parts.comments {
        push_comments(&mut out, id, &ticket.comments);
    }

    let parent_brief = parent.is_some_and(|(_, parent)| !parent.description.is_empty());
    let left_out = left_out(ticket, parent_brief, parts);
    if !left_out.is_empty() {
        out.push_str(&format!("\nNot shown: {left_out}.\n"));
    }

    out
}

/// Append a ticket's discussion to `out`, each comment under its position.
fn push_comments(out: &mut String, id: TicketId, comments: &[Comment]) {
    if comments.is_empty() {
        out.push_str("\nNo comments yet.\n");
        return;
    }

    out.push_str(&format!("\n## Comments ({})\n", comments.len()));
    for (index, comment) in comments.iter().enumerate() {
        // Stored as `#1`, shown with the id so the reference matches the
        // comment headings around it and can be quoted straight back.
        let re = comment.re.as_deref().map_or_else(String::new, |re| {
            re.strip_prefix('#').map_or_else(
                || format!(", replying to {re}"),
                |position| format!(", replying to {id}#{position}"),
            )
        });

        out.push_str(&format!(
            "\n### {}#{} \u{2014} {} at {}{re}\n\n{}\n",
            id,
            index + 1,
            comment.from,
            comment.date,
            comment.body
        ));
    }
}

/// The parts of `ticket` that `parts` leaves out and the ticket has, each with
/// the argument that shows it.
///
/// `parent_brief` is whether the ticket's parent has a description to show.
/// Empty when nothing was left out.
fn left_out(ticket: &Ticket, parent_brief: bool, parts: Parts) -> String {
    let mut left_out = vec![];
    if !parts.description && !ticket.description.is_empty() {
        left_out.push("the description (`description: true`)".to_owned());
    }
    if !parts.parent_description && parent_brief {
        left_out.push("the parent's description (`parent_description: true`)".to_owned());
    }
    if !parts.comments {
        match ticket.comments.len() {
            0 => {}
            1 => left_out.push("1 comment (`comments: true`)".to_owned()),
            count => left_out.push(format!("{count} comments (`comments: true`)")),
        }
    }

    left_out.join(" and ")
}

/// Read a ticket id from a tool argument.
///
/// An id carries letters, so anything that isn't a string is rejected outright
/// rather than coerced.
fn id_arg(value: &Value) -> Result<TicketId, String> {
    match value {
        Value::String(id) => id.parse::<TicketId>().map_err(|error| error.to_string()),
        other => Err(format!("`{other}` is not a ticket id.")),
    }
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
