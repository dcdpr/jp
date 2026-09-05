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

use std::{fs, io, path::MAIN_SEPARATOR};

use ::ticket::{
    Comment, Kind, Label, NewTicket, ParseError, Status, Ticket, TicketId, parse, render, store,
};
use camino::{Utf8Path, Utf8PathBuf};
use chrono::{Local, SecondsFormat, Utc};
use comfort::{
    DEFAULT_MAX_WIDTH,
    format::{FormatOptions, format_markdown_with},
};
use serde_json::Value;

use crate::{
    Context, Tool,
    util::{ToolResult, error, preview, unknown_tool},
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

            // Checked before the action split so a preview fails too: an
            // unattended formatter that errors fails the call ahead of the
            // approval prompt, which tells the assistant to fix the arguments
            // rather than asking the user about a call that cannot land.
            if title.trim().is_empty() {
                return error("`title` must not be empty.");
            }
            let labels = match resolve_labels(root, t.opt::<Vec<String>>("labels")?.as_deref())? {
                Ok(labels) => labels,
                Err(refusal) => return error(refusal),
            };

            if ctx.action.is_format_arguments() {
                let date = Local::now().format("%Y-%m-%d").to_string();
                return Ok(preview_create(
                    kind,
                    &title,
                    implements.as_deref(),
                    &labels,
                    body.as_deref(),
                    &date,
                )
                .into());
            }

            create(root, kind, &title, implements.as_deref(), &labels, body)
        }

        "label" => {
            let id = match id_arg(&t.req("id")?) {
                Ok(id) => id,
                Err(message) => return error(message),
            };
            label(
                root,
                id,
                &t.opt::<Vec<String>>("labels")?.unwrap_or_default(),
            )
        }

        "comment" => {
            let id = match id_arg(&t.req("id")?) {
                Ok(id) => id,
                Err(message) => return error(message),
            };
            let re = t.opt("re")?;
            let body = t.req::<String>("body")?;

            if body.trim().is_empty() {
                return error("`body` must not be empty.");
            }

            if ctx.action.is_format_arguments() {
                let date = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
                return preview_comment(root, id, re, &body, &date);
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
            let labels = t.opt::<Vec<String>>("labels")?.unwrap_or_default();
            list(root, status, kind, &labels)
        }

        _ => unknown_tool(t),
    }
}

fn create(
    root: &Utf8Path,
    kind: Kind,
    title: &str,
    implements: Option<&str>,
    labels: &[Label],
    body: Option<String>,
) -> ToolResult {
    let date = Local::now().format("%Y-%m-%d").to_string();
    let (id, path) = store::create(&dir(root), &NewTicket {
        kind,
        title: title.trim(),
        authors: HANDLE,
        date: &date,
        implements,
        labels,
        description: &body.unwrap_or_default(),
    })?;
    reflow(&path)?;

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
    labels: &[Label],
    body: Option<&str>,
    date: &str,
) -> String {
    preview(&render::ticket(&NewTicket {
        kind,
        title: title.trim(),
        authors: HANDLE,
        date,
        implements,
        labels,
        description: body.unwrap_or_default(),
    }))
}

/// Check labels against the board's vocabulary.
///
/// The outer error is the vocabulary file being unreadable, which is the
/// board's problem; the inner one is a label the board doesn't define, which is
/// the caller's and comes back as a tool error naming the known set.
fn resolve_labels(
    root: &Utf8Path,
    requested: Option<&[String]>,
) -> crate::Result<Result<Vec<Label>, String>> {
    // A call that names no labels doesn't read the vocabulary at all, so a
    // board with a broken `.labels.json` can still file unlabelled tickets.
    let Some(requested) = requested else {
        return Ok(Ok(vec![]));
    };

    Ok(store::vocabulary(&dir(root))?
        .resolve(requested)
        .map_err(|refusal| refusal.to_string()))
}

/// Replace a ticket's labels.
///
/// Checked against the ticket rather than against the vocabulary alone, so a
/// retired label the ticket already carries can be listed again and kept.
fn label(root: &Utf8Path, id: TicketId, requested: &[String]) -> ToolResult {
    let tickets = dir(root);
    let vocabulary = store::vocabulary(&tickets)?;

    match store::set_labels(&tickets, id, &vocabulary, requested) {
        Ok((_, applied)) if applied.is_empty() => Ok(format!("Cleared the labels on {id}.").into()),
        Ok((_, applied)) => Ok(format!("{id}: {}", ::ticket::labels::join(&applied)).into()),
        Err(store::Error::NoSuchTicket(_)) => error(format!("No {id}.")),
        Err(store::Error::Rejected(refusal)) => error(refusal.to_string()),
        Err(other) => Err(other.into()),
    }
}

fn comment(root: &Utf8Path, id: TicketId, re: Option<usize>, body: &str) -> ToolResult {
    let tickets = dir(root);
    let date = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    match store::append_comment(&tickets, id, HANDLE, &date, re, body.trim()) {
        Ok(position) => {
            reflow(&store::locate_ticket(&tickets, id)?)?;
            Ok(format!("Added {id}#{position}").into())
        }
        Err(store::Error::NoSuchTicket(_) | store::Error::NoSuchComment { .. }) => error(format!(
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
    root: &Utf8Path,
    id: TicketId,
    re: Option<usize>,
    body: &str,
    date: &str,
) -> ToolResult {
    let path = match store::locate_ticket(&dir(root), id) {
        Ok(path) => path,
        Err(store::Error::NoSuchTicket(_)) => return error(format!("No {id}.")),
        Err(other) => return Err(other.into()),
    };
    let document = fs::read_to_string(path)?;

    // The count comes from the same tolerant reader the append uses, so the
    // two agree on a file with a hand-mangled header.
    let count = parse::comment_count(&document);
    if let Some(position) = re
        && (position == 0 || position > count)
    {
        return error(format!("No comment #{position} on {id}."));
    }

    // The heading names the ticket, which its own file doesn't: the id is what
    // makes the preview readable next to the call that produced it.
    let heading = match parse::title(&document) {
        Some(title) => format!("# {id}: {title}"),
        None => format!("# {id}"),
    };

    let comment = Comment {
        from: HANDLE.to_owned(),
        date: date.to_owned(),
        re: re.map(|position| format!("#{position}")),
        body: body.to_owned(),
    };

    Ok(preview(&format!("{heading}\n\n{}", render::comment(&comment))).into())
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
        Ok(ticket) => {
            let rendered = render_ticket(entry.id, &ticket, &relative(root, &entry.path));
            // The terminal keys syntax highlighting off a leading code fence,
            // so the fence is what gets a ticket displayed as markdown rather
            // than as a flat wall of text.
            Ok(format!("```markdown\n{}\n```", rendered.trim_end()).into())
        }
        Err(problem) => error(format!("{id} is not a well-formed ticket: {problem}")),
    }
}

/// List the board, filtered by whatever the caller named.
///
/// Labels are matched as written on the ticket rather than through the
/// vocabulary, so a ticket carrying a label the board has since dropped can
/// still be found.
fn list(
    root: &Utf8Path,
    status: Option<Status>,
    kind: Option<Kind>,
    labels: &[String],
) -> ToolResult {
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
            && carries_every_label(ticket, labels)
    });

    Ok(render_list(&tickets, &unreadable).into())
}

/// Whether a ticket carries every one of `wanted`.
///
/// Requiring all of them rather than any composes with the other filters: each
/// argument narrows the listing.
fn carries_every_label(ticket: &Ticket, wanted: &[String]) -> bool {
    wanted.iter().all(|wanted| {
        ticket
            .metadata
            .labels
            .iter()
            .any(|label| label.eq_ignore_ascii_case(wanted.trim()))
    })
}

/// The ticket directory inside the workspace.
fn dir(root: &Utf8Path) -> Utf8PathBuf {
    root.join(store::DEFAULT_DIR)
}

/// Lay out a markdown file the way `comfort` does.
///
/// A body arrives as the model wrote it — one long line per paragraph — and
/// the repository's markdown carries semantic line breaks.
/// The options mirror the `fmt-markdown-ci` recipe in the justfile, so a ticket
/// written here is one CI accepts as it stands.
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
        let labels = match ticket.metadata.labels.as_slice() {
            [] => String::new(),
            labels => format!(" [{}]", labels.join(", ")),
        };

        out.push_str(&format!(
            "{id:<9} {status:<12} {kind:<8} {}{labels}{blocked}{comments}\n",
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
    if !metadata.labels.is_empty() {
        out.push_str(&format!("- **Labels**: {}\n", metadata.labels.join(", ")));
    }
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
