//! Writing ticket documents.
//!
//! Two shapes of write, both pure: [`ticket`] produces a new document, and
//! [`append_comment`] and [`set_metadata`] return an edited copy of an existing
//! one.
//!
//! Edits are deliberately narrow.
//! Comments are appended at the end and nothing above them moves; a metadata
//! write replaces the one line it targets.
//! A ticket is read far more often than it is written, and by hand as often as
//! by tooling, so the file is never round-tripped through the parser.

use crate::{Comment, Kind, Status, parse};

/// Render a new ticket, opened at `Todo`.
///
/// `implements` names the RFD the work comes from, for a ticket seeded from an
/// RFD's implementation plan.
#[must_use]
pub fn ticket(
    title: &str,
    kind: Kind,
    authors: &str,
    date: &str,
    implements: Option<&str>,
    description: &str,
) -> String {
    let mut out = format!("# {title}\n\n");
    out.push_str(&format!("- **Status**: {}\n", Status::Todo));
    out.push_str(&format!("- **Kind**: {kind}\n"));
    out.push_str(&format!("- **Authors**: {authors}\n"));
    out.push_str(&format!("- **Date**: {date}\n"));
    if let Some(rfd) = implements {
        out.push_str(&format!("- **Implements**: {rfd}\n"));
    }

    let description = description.trim();
    if !description.is_empty() {
        out.push('\n');
        out.push_str(description);
        out.push('\n');
    }

    out
}

/// Append a comment, returning the new document.
///
/// The `## Comments` heading is written before the first comment and never
/// again.
#[must_use]
pub fn append_comment(document: &str, comment: &Comment) -> String {
    let mut out = document.trim_end().to_owned();

    if parse::comment_count(document) == 0 {
        out.push_str("\n\n## Comments");
    }
    push_comment(&mut out, comment);

    out
}

/// Replace a ticket's title, description, and comments, keeping its metadata
/// block verbatim.
///
/// Returns `None` when the document has no metadata block to keep.
/// This is the write an import performs: upstream owns the content, the
/// repository owns the metadata.
#[must_use]
pub fn replace_content(
    document: &str,
    title: &str,
    description: &str,
    comments: &[Comment],
) -> Option<String> {
    let header = parse::metadata_range(document)?;
    let lines: Vec<&str> = document.lines().collect();

    let mut out = format!("# {title}\n\n");
    for index in header {
        out.push_str(lines[index]);
        out.push('\n');
    }

    let description = description.trim();
    if !description.is_empty() {
        out.push_str(&format!("\n{description}\n"));
    }

    if !comments.is_empty() {
        out = out.trim_end().to_owned();
        out.push_str("\n\n## Comments");
        for comment in comments {
            push_comment(&mut out, comment);
        }
    }

    Some(out)
}

/// Write one comment: the separator that opens it, its metadata, and its body.
///
/// A comment carries its own separator, so appending one never has to touch
/// what is already there.
/// Expects `out` not to end in a newline.
fn push_comment(out: &mut String, comment: &Comment) {
    out.push_str("\n\n-----\n\n");
    out.push_str(&format!("- **From**: {}\n", comment.from));
    out.push_str(&format!("- **Date**: {}\n", comment.date));
    if let Some(re) = &comment.re {
        out.push_str(&format!("- **Re**: {re}\n"));
    }
    out.push('\n');
    out.push_str(comment.body.trim());
    out.push('\n');
}

/// Un-embed `id` from a document that names itself.
///
/// Tickets written before RFD 102 carried their id in two places inside the
/// file: the title heading, and the reply target of every comment answering
/// another on the same ticket.
/// Both become the id-free form, so the filename is the only thing naming the
/// ticket.
///
/// `id` is matched exactly, so a title that merely contains a colon — `Fix:
/// the thing` — and a reply naming a *different* ticket are both left alone.
/// A document with nothing to convert comes back unchanged.
#[must_use]
pub fn strip_ids(document: &str, id: &str) -> String {
    let mut lines: Vec<String> = document.lines().map(ToOwned::to_owned).collect();

    if let Some(index) = lines.iter().position(|line| !line.trim().is_empty())
        && let Some(title) = lines[index]
            .strip_prefix("# ")
            .and_then(|rest| rest.strip_prefix(id))
            .and_then(|rest| rest.strip_prefix(':'))
    {
        lines[index] = format!("# {}", title.trim());
    }

    for line in &mut lines {
        let replacement = parse::meta_line(line)
            .filter(|(key, _)| key.eq_ignore_ascii_case("re"))
            .and_then(|(_, value)| value.strip_prefix(id))
            .and_then(|rest| rest.strip_prefix('#'))
            .map(|position| format!("- **Re**: #{position}"));

        if let Some(replacement) = replacement {
            *line = replacement;
        }
    }

    let mut out = lines.join("\n");
    if document.ends_with('\n') {
        out.push('\n');
    }

    out
}

/// Set a field in the ticket's metadata block, returning the new document.
///
/// An existing field is replaced in place; one the ticket doesn't carry yet
/// joins the end of the block.
/// Returns `None` when the document has no metadata block at all.
///
/// Only the header block is considered, so a `- **Status**: Done` line quoted
/// in a comment is left alone.
#[must_use]
pub fn set_metadata(document: &str, key: &str, value: &str) -> Option<String> {
    let header = parse::metadata_range(document)?;
    let mut lines: Vec<String> = document.lines().map(ToOwned::to_owned).collect();
    let line = format!("- **{key}**: {value}");

    let existing = header.clone().find(|&i| {
        parse::meta_line(&lines[i]).is_some_and(|(found, _)| found.eq_ignore_ascii_case(key))
    });
    match existing {
        Some(index) => lines[index] = line,
        None => lines.insert(header.end, line),
    }

    let mut out = lines.join("\n");
    if document.ends_with('\n') {
        out.push('\n');
    }

    Some(out)
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
