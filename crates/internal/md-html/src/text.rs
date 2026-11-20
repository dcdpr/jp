//! Plain-text HTML rendering with per-line source byte ranges.

use std::fmt::{self, Write as _};

use comrak::html::escape;

use crate::{Rendered, position::split_lines};

/// Render plain text as preformatted HTML (`<pre>`), wrapping each line in a
/// source-mapped `<span>`.
///
/// Each line (including empty lines) is wrapped in `<span data-s="S"
/// data-e="E">` where the text matches `source[S..E]` character for character,
/// matching the format produced by [`render`].
/// Line endings are placed outside the spans, and the trailing empty segment
/// after a final newline is omitted.
///
/// [`render`]: crate::render
#[must_use]
pub fn render_text(source: &str) -> Rendered {
    let mut html = String::from("<pre>");
    // Writing to a `String` cannot fail.
    let _ = write_lines(&mut html, source);
    html.push_str("</pre>\n");

    Rendered { html }
}

fn write_lines(out: &mut String, source: &str) -> fmt::Result {
    let mut start = 0;
    for (line, ending) in split_lines(source) {
        // `split_lines` always ends with a trailing `(rest, "")` element;
        // skip it when `source` is empty or ends with a newline.
        if line.is_empty() && ending.is_empty() {
            break;
        }

        let end = start + line.len();
        write!(out, "<span data-s=\"{start}\" data-e=\"{end}\">")?;
        escape(out, line)?;
        out.push_str("</span>");
        if !ending.is_empty() {
            out.push('\n');
        }
        start = end + ending.len();
    }

    Ok(())
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
