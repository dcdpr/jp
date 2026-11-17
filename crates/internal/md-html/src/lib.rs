//! Markdown rendered to HTML, with the source byte range of every run of
//! rendered text.
//!
//! [`render`] parses Markdown with comrak and wraps each run of text it renders
//! in a `<span>` of one of three kinds:
//!
//! - `<span data-s="S" data-e="E">`: the text is `source[S..E]`, character for
//!   character, so an offset into the text is an offset from `S`.
//! - `<span data-s="S" data-e="E" data-atom>`: the text stands for
//!   `source[S..E]` as a whole: the `|` of an escaped pipe (`\|`) in a table
//!   cell, or the title comrak gives an alert for its marker, such as "Note"
//!   for `[!NOTE]`.
//! - `<span class="unmapped">`: the source range comrak reports for the text
//!   does not hold the text, so the span claims none.
//!
//! `S` and `E` are UTF-8 byte offsets into the string passed to [`render`], and
//! `E` is exclusive.
//! Markdown syntax renders no text and is in no span: emphasis markers,
//! backticks, link destinations, list markers.
//! Neither is whitespace the renderer puts between spans: line endings, and the
//! space that stands for a line ending inside a code span.
//!
//! Raw HTML in the source is escaped and rendered as text, and a link or image
//! destination with a `javascript:`, `vbscript:`, `file:`, or non-image `data:`
//! scheme is dropped.
//!
//! [`Lines`] converts between byte offsets and 1-based line and column
//! positions.
//!
//! See `docs/rfd/124-source-mapped-markdown-rendering.md`.

mod formatter;
mod position;
mod range;
mod span;

use comrak::{
    Arena, Options, html::format_document_with_formatter, options::Plugins, parse_document,
};

pub use crate::position::{LineColumn, Lines};

/// Markdown rendered to HTML by [`render`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// The HTML fragment the Markdown renders as.
    pub html: String,
}

/// Render Markdown to HTML whose text spans carry the byte range of `source`
/// they came from.
///
/// `source` is CommonMark with GitHub's tables, strikethrough, autolinks, task
/// lists, and alerts.
/// Offsets are into `source` exactly as given: a caller that changes the
/// Markdown before rendering it maps the offsets back to the original itself.
#[must_use]
pub fn render(source: &str) -> Rendered {
    let options = options();
    let arena = Arena::new();
    let root = parse_document(&arena, source, &options);
    let lines = Lines::new(source);

    let mut html = String::new();
    // Writing to a `String` cannot fail.
    let _ = format_document_with_formatter(
        root,
        &options,
        &mut html,
        &Plugins::default(),
        formatter::format_node,
        &lines,
    );

    Rendered { html }
}

fn options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.alerts = true;
    // A backslash escape becomes a node of its own, so the text on either side
    // keeps a source position that leaves the backslash out.
    options.parse.escaped_char_spans = true;

    options
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
