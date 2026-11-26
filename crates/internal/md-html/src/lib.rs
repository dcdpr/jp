//! Markdown-to-HTML rendering with source byte ranges on every rendered text
//! span.
//!
//! [`render`] parses Markdown with comrak and wraps each run of rendered text
//! in one of three `<span>` variants:
//!
//! - `<span data-s="S" data-e="E">`: the rendered text matches `source[S..E]`
//!   character for character, so a byte offset into the text maps directly to
//!   an offset from `S`.
//! - `<span data-s="S" data-e="E" data-atom>`: the rendered text represents
//!   `source[S..E]` as an indivisible unit, such as `|` rendered from an
//!   escaped pipe (`\|`) in a table cell or the default title ("Note")
//!   synthesized for an `[!NOTE]` alert marker.
//! - `<span class="unmapped">`: the source range reported by comrak does not
//!   match the rendered text, so no byte range is attached.
//!
//! `S` and `E` are UTF-8 byte offsets into the string passed to [`render`], and
//! `E` is exclusive.
//! Markdown syntax (emphasis markers, backticks, link destinations, list
//! markers) produces no rendered text and is not wrapped in a span.
//! Whitespace inserted by the renderer between spans (line endings, and the
//! space that replaces a line ending inside a wrapped code span) is also
//! outside any span.
//!
//! Raw HTML in the source is escaped and rendered as text, and link or image
//! destinations with a `javascript:`, `vbscript:`, `file:`, or non-image
//! `data:` scheme are stripped.
//!
//! [`render_with_anchors`] also gives headings the `id`s its caller names, for
//! `#fragment` links to land on.
//!
//! [`render_text`] renders plain text as preformatted HTML (`<pre>`), wrapping
//! each line in a `<span>` with its source byte range.
//!
//! [`Lines`] converts between byte offsets and 1-based line and column
//! positions.
//!
//! See `docs/rfd/124-source-mapped-markdown-rendering.md`.

mod formatter;
mod position;
mod range;
mod span;
mod text;

use std::collections::BTreeMap;

use comrak::{
    Arena, Options, html::format_document_with_formatter, options::Plugins, parse_document,
};

pub use crate::{
    position::{LineColumn, Lines},
    text::render_text,
};

/// Text rendered to HTML by [`render`] or [`render_text`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub html: String,
}

/// Render Markdown to HTML with source byte ranges on every text span.
///
/// `source` is parsed as CommonMark with GitHub tables, strikethrough,
/// autolinks, task lists, and alerts.
/// Offsets index into `source` as passed; callers that transform the Markdown
/// before rendering must map offsets back to the original input themselves.
#[must_use]
pub fn render(source: &str) -> Rendered {
    render_with_anchors(source, &BTreeMap::new())
}

/// Render Markdown as [`render`] does, with an `id` on the headings `anchors`
/// names.
///
/// `anchors` maps the 0-based index of the line a heading starts on to the `id`
/// that heading gets; a heading on any other line gets none.
/// The caller decides the ids, and keeps them unique in the page the HTML goes
/// into.
#[must_use]
pub fn render_with_anchors(source: &str, anchors: &BTreeMap<usize, String>) -> Rendered {
    let options = options();
    let arena = Arena::new();
    let root = parse_document(&arena, source, &options);
    let document = formatter::Document {
        lines: Lines::new(source),
        anchors,
    };

    let mut html = String::new();
    // Writing to a `String` cannot fail.
    let _ = format_document_with_formatter(
        root,
        &options,
        &mut html,
        &Plugins::default(),
        formatter::format_node,
        &document,
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
    // Emit backslash escapes as separate AST nodes so adjacent text nodes have
    // source positions that exclude the backslash.
    options.parse.escaped_char_spans = true;

    options
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
