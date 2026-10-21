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
    Comment, Kind, Labels, NewTicket, ParseError, Selector, Status, Ticket, TicketId,
    board::{self, Order},
    parse, render, store,
};

use crate::options::Options;

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
        "list" => list(&tickets, &args),
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
    let Ok(kind) = args.required::<String>("kind")?.parse::<Kind>() else {
        return refuse("`kind` must be one of: bug, feature, chore.");
    };
    let title = args.required::<String>("title")?;
    let implements = args.optional::<String>("implements")?;
    let body = args.optional::<String>("body")?;

    // Checked before the action split so a preview fails too: an unattended
    // formatter that errors fails the call ahead of the approval prompt, which
    // tells the assistant to fix the arguments rather than asking the user
    // about a call that cannot land.
    if title.trim().is_empty() {
        return refuse("`title` must not be empty.");
    }
    let labels = match resolve_labels(
        tickets.options,
        args.optional::<Vec<String>>("labels")?.as_deref(),
    )? {
        Ok(labels) => labels,
        Err(refusal) => return refuse(refusal),
    };

    let limit = option_or(&call.options, "max_title_length", DEFAULT_MAX_TITLE_LENGTH);
    let title = match fitting_title(&title, &call.answers, limit) {
        Ok(title) => title,
        Err(outcome) => return outcome,
    };

    let date = Local::now().format("%Y-%m-%d").to_string();
    let new = NewTicket {
        kind,
        title: &title,
        authors: tickets.author,
        date: &date,
        implements: implements.as_deref(),
        labels: &labels,
        description: body.as_deref().unwrap_or_default(),
    };

    if preview {
        return Ok(preview_create(&new).into());
    }

    let (id, path) = store::create(&tickets.dir, &new)?;
    reflow(&path)?;

    Ok(format!("Created {id} at {}", tickets.relative(&path)).into())
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

/// Check labels against the board's vocabulary.
///
/// The outer error is the vocabulary being unreadable, which is the board's
/// problem; the inner one is a label the board doesn't define, which is the
/// caller's and comes back as a refusal naming the known set.
fn resolve_labels(
    options: &Options,
    requested: Option<&[String]>,
) -> Result<Result<Labels, String>, Error> {
    // A call that names no labels doesn't read the vocabulary at all, so a
    // board with a broken vocabulary can still file unlabelled tickets.
    let Some(requested) = requested else {
        return Ok(Ok(Labels::default()));
    };

    Ok(options
        .vocabulary()?
        .resolve(requested)
        .map_err(|refusal| refusal.to_string()))
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

/// Change any of a ticket's title, description, kind, status, and labels in one
/// call, leaving the rest as it is.
///
/// Every argument is checked before anything is written.
/// The labels, which the vocabulary can still refuse, are written first, so a
/// refused call changes nothing; the title last, since it moves the file.
fn edit(tickets: &Tickets<'_>, args: &Arguments<'_>) -> Answer {
    let id = match args.id()? {
        Ok(id) => id,
        Err(refusal) => return refuse(refusal),
    };
    let title = args.optional::<String>("title")?;
    let description = args.optional::<String>("description")?;
    let kind = match args
        .optional::<String>("kind")?
        .map(|kind| kind.parse::<Kind>())
    {
        Some(Err(_)) => return refuse("`kind` must be one of: bug, feature, chore."),
        Some(Ok(kind)) => Some(kind),
        None => None,
    };
    let status = match args
        .optional::<String>("status")?
        .map(|status| status.parse::<Status>())
    {
        Some(Err(_)) => return refuse("`status` must be one of: todo, in progress, done."),
        Some(Ok(status)) => Some(status),
        None => None,
    };
    let labels = args.optional::<Vec<String>>("labels")?;

    if title.is_none()
        && description.is_none()
        && kind.is_none()
        && status.is_none()
        && labels.is_none()
    {
        return refuse(
            "Name what to change: `title`, `description`, `kind`, `status`, or `labels`.",
        );
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

    match store::locate_ticket(&tickets.dir, id) {
        Ok(_) => {}
        Err(store::Error::NoSuchTicket(_)) => return refuse(format!("No {id}.")),
        Err(other) => return Err(other.into()),
    }

    let mut changed = vec![];
    if let Some(requested) = &labels {
        let vocabulary = tickets.options.vocabulary()?;
        match store::set_labels(&tickets.dir, id, &vocabulary, requested) {
            Ok((_, applied)) if applied.is_empty() => changed.push("labels cleared".to_owned()),
            Ok((_, applied)) => changed.push(format!("labels: {applied}")),
            Err(store::Error::Rejected(refusal)) => return refuse(refusal.to_string()),
            Err(other) => return Err(other.into()),
        }
    }
    if let Some(kind) = kind {
        store::set_field(&tickets.dir, id, "Kind", &kind.to_string())?;
        changed.push(format!("kind: {kind}"));
    }
    if let Some(status) = status {
        let (_, previous) = store::set_status(&tickets.dir, id, status)?;
        changed.push(if previous == status {
            format!("status: already {status}")
        } else {
            format!("status: {previous} -> {status}")
        });
    }
    if title.is_some() || description.is_some() {
        let store::Edited { from, to } = store::edit(
            &tickets.dir,
            id,
            title.as_deref().map(str::trim),
            description.as_deref(),
        )?;
        if description.is_some() {
            reflow(&to)?;
            changed.push("description replaced".to_owned());
        }
        if title.is_some() {
            changed.push(if from == to {
                format!("title changed, still at {}", tickets.relative(&to))
            } else {
                format!(
                    "title changed, moved {} -> {}",
                    tickets.relative(&from),
                    tickets.relative(&to)
                )
            });
        }
    }

    let mut text = format!("Edited {id}:\n");
    for change in changed {
        text.push_str(&format!("- {change}\n"));
    }

    Ok(text.into())
}

/// Read one ticket: its metadata, and the description and comments when the
/// call asks for them.
fn show(tickets: &Tickets<'_>, args: &Arguments<'_>) -> Answer {
    let id = match args.id()? {
        Ok(id) => id,
        Err(refusal) => return refuse(refusal),
    };
    let parts = Parts {
        description: args.optional("description")?.unwrap_or(false),
        comments: args.optional("comments")?.unwrap_or(false),
    };

    let entries = store::list(&tickets.dir)?;
    let Some(entry) = entries.into_iter().find(|entry| {
        entry
            .path
            .file_name()
            .is_some_and(|name| name.starts_with(&id.file_prefix()))
    }) else {
        return refuse(format!("No {id}."));
    };

    match entry.ticket {
        Ok(ticket) => {
            let rendered = render_ticket(entry.id, &ticket, &tickets.relative(&entry.path), parts);
            // The terminal keys syntax highlighting off a leading code fence,
            // so the fence is what gets a ticket displayed as markdown rather
            // than as a flat wall of text.
            Ok(format!("```markdown\n{}\n```", rendered.trim_end()).into())
        }
        Err(problem) => refuse(format!("{id} is not a well-formed ticket: {problem}")),
    }
}

/// List the board, filtered by whatever the caller named.
///
/// Tickets come in board order: Todo, In Progress, then Done, each column top
/// first, so the first Todo line is the next thing to work on.
/// Labels are matched as written on the ticket rather than through the
/// vocabulary, so a ticket carrying a label the board has since dropped can
/// still be found.
fn list(tickets: &Tickets<'_>, args: &Arguments<'_>) -> Answer {
    let status = match args
        .optional::<String>("status")?
        .map(|s| s.parse::<Status>())
    {
        Some(Err(_)) => return refuse("`status` must be one of: todo, in progress, done."),
        Some(Ok(status)) => Some(status),
        None => None,
    };
    let kind = match args.optional::<String>("kind")?.map(|k| k.parse::<Kind>()) {
        Some(Err(_)) => return refuse("`kind` must be one of: bug, feature, chore."),
        Some(Ok(kind)) => Some(kind),
        None => None,
    };
    let labels = args.optional::<Vec<String>>("labels")?.unwrap_or_default();

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
            && kind.is_none_or(|kind| kind == ticket.metadata.kind)
            && ticket.metadata.labels.matches(&selectors)
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

    Ok(render_list(&in_board_order(&found, &order), &unreadable).into())
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

/// Render the board as one line per ticket.
fn render_list(tickets: &[(TicketId, &Ticket)], unreadable: &[String]) -> String {
    let mut out = String::new();

    if tickets.is_empty() {
        out.push_str("No tickets match.\n");
    }
    for (id, ticket) in tickets {
        let id = id.to_string();
        let status = ticket.metadata.status.to_string();
        let kind = ticket.metadata.kind.to_string();
        let comments = match ticket.comments.len() {
            0 => String::new(),
            count => format!(" ({count} comments)"),
        };
        let labels = if ticket.metadata.labels.is_empty() {
            String::new()
        } else {
            format!(" [{}]", ticket.metadata.labels)
        };

        out.push_str(&format!(
            "{id:<9} {status:<12} {kind:<8} {}{labels}{comments}\n",
            ticket.title
        ));
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
}

/// Render one ticket's metadata, and the parts `parts` asks for.
///
/// Comments are numbered so a reply can name its target.
/// A part left out that the ticket has is named at the end, with the argument
/// that shows it, so the reader knows there is more without asking.
fn render_ticket(id: TicketId, ticket: &Ticket, path: &str, parts: Parts) -> String {
    let metadata = &ticket.metadata;
    // The rendered view names the ticket even though the file doesn't: the id
    // is what a reply or a later reference has to quote.
    let mut out = format!("# {id}: {}\n\n", ticket.title);

    out.push_str(&format!("- **Path**: {path}\n"));
    out.push_str(&format!("- **Status**: {}\n", metadata.status));
    out.push_str(&format!("- **Kind**: {}\n", metadata.kind));
    for token in metadata.labels.to_tokens() {
        out.push_str(&format!("- **Label**: {token}\n"));
    }
    out.push_str(&format!("- **Authors**: {}\n", metadata.authors));
    out.push_str(&format!("- **Date**: {}\n", metadata.date));
    for (label, value) in [
        ("Implements", &metadata.implements),
        ("Promoted to", &metadata.promoted_to),
        ("GitHub", &metadata.github),
    ] {
        if let Some(value) = value {
            out.push_str(&format!("- **{label}**: {value}\n"));
        }
    }

    if parts.description && !ticket.description.is_empty() {
        out.push_str(&format!("\n{}\n", ticket.description));
    }

    if parts.comments {
        push_comments(&mut out, id, &ticket.comments);
    }

    let left_out = left_out(ticket, parts);
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
/// Empty when nothing was left out.
fn left_out(ticket: &Ticket, parts: Parts) -> String {
    let mut left_out = vec![];
    if !parts.description && !ticket.description.is_empty() {
        left_out.push("the description (`description: true`)".to_owned());
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
        Value::String(id) => id.parse().map_err(|error: ParseError| error.to_string()),
        other => Err(format!("`{other}` is not a ticket id.")),
    }
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
