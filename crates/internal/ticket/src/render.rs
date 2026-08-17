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

use crate::{Comment, Kind, Status, TicketId, parse};

/// Render a new ticket, opened at `Todo`.
///
/// `implements` names the RFD the work comes from, for a ticket seeded from an
/// RFD's implementation plan.
#[must_use]
pub fn ticket(
    id: TicketId,
    title: &str,
    kind: Kind,
    authors: &str,
    date: &str,
    implements: Option<&str>,
    description: &str,
) -> String {
    let mut out = format!("# {id}: {title}\n\n");
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
    id: TicketId,
    title: &str,
    description: &str,
    comments: &[Comment],
) -> Option<String> {
    let header = parse::metadata_range(document)?;
    let lines: Vec<&str> = document.lines().collect();

    let mut out = format!("# {id}: {title}\n\n");
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

/// Render one comment: the separator that opens it, its metadata, and its body.
///
/// A comment carries its own separator, so appending one never has to touch
/// what is already there.
#[must_use]
pub fn comment(comment: &Comment) -> String {
    let mut out = String::from("-----\n\n");
    out.push_str(&format!("- **From**: {}\n", comment.from));
    out.push_str(&format!("- **Date**: {}\n", comment.date));
    if let Some(re) = &comment.re {
        out.push_str(&format!("- **Re**: {re}\n"));
    }
    out.push('\n');
    out.push_str(comment.body.trim());
    out.push('\n');

    out
}

/// Append one comment to a document.
///
/// Expects `out` not to end in a newline.
fn push_comment(out: &mut String, entry: &Comment) {
    out.push_str("\n\n");
    out.push_str(&comment(entry));
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
