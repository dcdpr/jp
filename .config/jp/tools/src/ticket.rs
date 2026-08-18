//! The `ticket_*` tools: create, read, comment on, and close the work items
//! under `docs/ticket/`.
//!
//! The format lives in the `ticket` crate; this module is the tool surface over
//! it.
//! Tickets the assistant writes are attributed to `jp`, and comments are
//! rendered with their 1-based positions so a reply can name the comment it
//! answers.
//!
//! `ticket_create` and `ticket_comment` also answer the format-arguments
//! action, previewing the document they are about to write in the shape it
//! takes on disk.

use std::{fs, path::MAIN_SEPARATOR};

// The leading `::` picks the crate over this module, which shares its name.
use ::ticket::{Comment, Kind, ParseError, Status, Ticket, TicketId, parse, render, store};
use camino::{Utf8Path, Utf8PathBuf};
use chrono::{Local, SecondsFormat, Utc};
use jp_md::format::Formatter;
use serde_json::Value;

use crate::{
    Context, Tool,
    util::{ToolResult, error, unknown_tool},
};

/// The handle tickets and comments written by the assistant carry.
const HANDLE: &str = "jp";

#[expect(
    clippy::needless_pass_by_value,
    reason = "consistent with other module run fns"
)]
pub fn run(ctx: Context, t: Tool) -> ToolResult {
    let root = ctx.root.as_path();

    match t.name.trim_start_matches("ticket_") {
        "create" => {
            let Ok(kind) = t.req::<String>("kind")?.parse::<Kind>() else {
                return error("`kind` must be one of: bug, feature, chore.");
            };
            let title = t.req::<String>("title")?;
            let implements = t.opt::<String>("implements")?;
            let body = t.opt::<String>("body")?;

            if ctx.action.is_format_arguments() {
                let date = Local::now().format("%Y-%m-%d").to_string();
                return Ok(preview_create(
                    kind,
                    &title,
                    implements.as_deref(),
                    body.as_deref(),
                    &date,
                )
                .into());
            }

            create(root, kind, &title, implements.as_deref(), body)
        }

        "comment" => {
            let id = match id_arg(&t.req("id")?) {
                Ok(id) => id,
                Err(message) => return error(message),
            };
            let re = t.opt("re")?;
            let body = t.req::<String>("body")?;

            if ctx.action.is_format_arguments() {
                let date = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
                return Ok(preview_comment(root, id, re, &body, &date).into());
            }

            comment(root, id, re, &body)
        }

        "close" => match id_arg(&t.req("id")?) {
            Ok(id) => close(root, id),
            Err(message) => error(message),
        },

        "show" => match id_arg(&t.req("id")?) {
            Ok(id) => show(root, id),
            Err(message) => error(message),
        },

        "list" => {
            let status = match t.opt::<String>("status")?.map(|s| s.parse::<Status>()) {
                Some(Err(_)) => {
                    return error("`status` must be one of: todo, in progress, done.");
                }
                Some(Ok(status)) => Some(status),
                None => None,
            };
            let kind = match t.opt::<String>("kind")?.map(|k| k.parse::<Kind>()) {
                Some(Err(_)) => return error("`kind` must be one of: bug, feature, chore."),
                Some(Ok(kind)) => Some(kind),
                None => None,
            };
            list(root, status, kind)
        }

        _ => unknown_tool(t),
    }
}

fn create(
    root: &Utf8Path,
    kind: Kind,
    title: &str,
    implements: Option<&str>,
    body: Option<String>,
) -> ToolResult {
    if title.trim().is_empty() {
        return error("`title` must not be empty.");
    }

    let date = Local::now().format("%Y-%m-%d").to_string();
    let (id, path) = store::create(
        &dir(root),
        kind,
        title.trim(),
        HANDLE,
        &date,
        implements,
        &body.unwrap_or_default(),
    )?;

    Ok(format!("Created {id} at {}", relative(root, &path)).into())
}

/// Render the ticket file `create` is about to write.
///
/// The id is left out because the file doesn't carry one: it is drawn when the
/// ticket is claimed, and the result names it.
fn preview_create(
    kind: Kind,
    title: &str,
    implements: Option<&str>,
    body: Option<&str>,
    date: &str,
) -> String {
    preview(&render::ticket(
        title.trim(),
        kind,
        HANDLE,
        date,
        implements,
        body.unwrap_or_default(),
    ))
}

fn comment(root: &Utf8Path, id: TicketId, re: Option<usize>, body: &str) -> ToolResult {
    if body.trim().is_empty() {
        return error("`body` must not be empty.");
    }

    let date = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    match store::append_comment(&dir(root), id, HANDLE, &date, re, body.trim()) {
        Ok(position) => Ok(format!("Added {id}#{position}").into()),
        Err(store::Error::NoSuchTicket(_) | store::Error::NoSuchComment { .. }) => error(format!(
            "No {id}, or no comment #{} on it.",
            re.unwrap_or(0)
        )),
        Err(other) => Err(other.into()),
    }
}

/// Render the comment block `comment` is about to append, under the heading of
/// the ticket it lands on.
fn preview_comment(
    root: &Utf8Path,
    id: TicketId,
    re: Option<usize>,
    body: &str,
    date: &str,
) -> String {
    // The heading names the ticket, which its own file doesn't: the id is what
    // makes the preview readable next to the call that produced it.
    let heading = match title_of(root, id) {
        Some(title) => format!("# {id}: {title}"),
        None => format!("# {id}\n\n\u{26a0} No {id}; this call will fail."),
    };

    let comment = Comment {
        from: HANDLE.to_owned(),
        date: date.to_owned(),
        re: re.map(|position| format!("#{position}")),
        body: body.to_owned(),
    };

    preview(&format!("{heading}\n\n{}", render::comment(&comment)))
}

fn close(root: &Utf8Path, id: TicketId) -> ToolResult {
    match store::close(&dir(root), id) {
        Ok((_, Status::Done)) => Ok(format!("{id} was already Done.").into()),
        Ok((_, previous)) => Ok(format!("{id}: {previous} -> Done").into()),
        Err(store::Error::NoSuchTicket(id)) => error(format!("No {id}.")),
        Err(other) => Err(other.into()),
    }
}

fn show(root: &Utf8Path, id: TicketId) -> ToolResult {
    let entries = store::list(&dir(root))?;
    let Some(entry) = entries.into_iter().find(|entry| {
        entry
            .path
            .file_name()
            .is_some_and(|name| name.starts_with(&id.file_prefix()))
    }) else {
        return error(format!("No {id}."));
    };

    match entry.ticket {
        Ok(ticket) => Ok(render_ticket(entry.id, &ticket, &relative(root, &entry.path)).into()),
        Err(problem) => error(format!("{id} is not a well-formed ticket: {problem}")),
    }
}

fn list(root: &Utf8Path, status: Option<Status>, kind: Option<Kind>) -> ToolResult {
    let entries = store::list(&dir(root))?;

    let mut tickets = vec![];
    let mut unreadable = vec![];
    for entry in &entries {
        match &entry.ticket {
            Ok(ticket) => tickets.push((entry.id, ticket)),
            Err(problem) => unreadable.push(format!("{}: {problem}", relative(root, &entry.path))),
        }
    }
    tickets.retain(|(_, ticket)| {
        status.is_none_or(|status| status == ticket.metadata.status)
            && kind.is_none_or(|kind| kind == ticket.metadata.kind)
    });

    Ok(render_list(&tickets, &unreadable).into())
}

/// The ticket directory inside the workspace.
fn dir(root: &Utf8Path) -> Utf8PathBuf {
    root.join(store::DEFAULT_DIR)
}

/// A path the model can hand straight to `fs_read_file`.
///
/// Separators are always `/`, on every platform: the path travels through tool
/// results into the conversation, and `fs_read_file` takes forward slashes
/// everywhere.
fn relative(root: &Utf8Path, path: &Utf8Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .as_str()
        .replace(MAIN_SEPARATOR, "/")
}

/// The title of a ticket on disk, if it is there and readable.
fn title_of(root: &Utf8Path, id: TicketId) -> Option<String> {
    let path = store::locate_ticket(&dir(root), id).ok()?;
    let document = fs::read_to_string(path).ok()?;

    parse::document(&document).ok().map(|ticket| ticket.title)
}

/// Style a document for the terminal as a tool-call preview.
///
/// The document is quoted first, so the transcript carries a marker down the
/// whole preview and the reader can see where the ticket ends and the
/// conversation resumes.
/// Falls back to the unstyled source if the markdown can't be formatted.
fn preview(document: &str) -> String {
    let mut quoted = String::with_capacity(document.len() * 2);
    for line in document.lines() {
        quoted.push('>');
        if !line.is_empty() {
            quoted.push(' ');
            quoted.push_str(line);
        }
        quoted.push('\n');
    }

    Formatter::new()
        .format_terminal(&quoted)
        .unwrap_or_else(|_| quoted.clone())
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
        let blocked = ticket
            .metadata
            .blocked_by
            .as_deref()
            .map_or_else(String::new, |by| format!(" [blocked by {by}]"));

        out.push_str(&format!(
            "{id:<9} {status:<12} {kind:<8} {}{blocked}{comments}\n",
            ticket.title
        ));
    }

    if !unreadable.is_empty() {
        out.push_str("\nUnreadable ticket files:\n");
        for problem in unreadable {
            out.push_str(&format!("- {problem}\n"));
        }
    }

    out
}

/// Render one ticket, numbering the comments so a reply can name its target.
fn render_ticket(id: TicketId, ticket: &Ticket, path: &str) -> String {
    let metadata = &ticket.metadata;
    // The rendered view names the ticket even though the file doesn't: the id
    // is what a `Blocked by` or a later reference has to quote.
    let mut out = format!("# {id}: {}\n\n", ticket.title);

    out.push_str(&format!("- **Path**: {path}\n"));
    out.push_str(&format!("- **Status**: {}\n", metadata.status));
    out.push_str(&format!("- **Kind**: {}\n", metadata.kind));
    out.push_str(&format!("- **Authors**: {}\n", metadata.authors));
    out.push_str(&format!("- **Date**: {}\n", metadata.date));
    for (label, value) in [
        ("Blocked by", &metadata.blocked_by),
        ("Implements", &metadata.implements),
        ("Promoted to", &metadata.promoted_to),
        ("GitHub", &metadata.github),
    ] {
        if let Some(value) = value {
            out.push_str(&format!("- **{label}**: {value}\n"));
        }
    }

    if !ticket.description.is_empty() {
        out.push_str(&format!("\n{}\n", ticket.description));
    }

    if ticket.comments.is_empty() {
        out.push_str("\nNo comments yet.\n");
        return out;
    }

    out.push_str(&format!("\n## Comments ({})\n", ticket.comments.len()));
    for (index, comment) in ticket.comments.iter().enumerate() {
        // Stored as `#1`, shown with the id so the reference matches the
        // comment headings around it and can be quoted straight back.
        let re = comment.re.as_deref().map_or_else(String::new, |re| {
            re.strip_prefix('#').map_or_else(
                || format!(", replying to {re}"),
                |position| format!(", replying to {id}#{position}"),
            )
        });

        out.push_str(&format!(
            "\n### {}#{} — {} at {}{re}\n\n{}\n",
            id,
            index + 1,
            comment.from,
            comment.date,
            comment.body
        ));
    }

    out
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
#[path = "ticket_tests.rs"]
mod tests;
