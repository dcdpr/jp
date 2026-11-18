//! Plain text rendered to HTML, each line in a span of its own.

use std::fmt::{self, Write as _};

use comrak::html::escape;

use crate::{Rendered, position::split_lines};

/// Render plain text to HTML as preformatted text, each line in a span of its
/// own.
///
/// Each line, an empty one included, is a `<span data-s="S" data-e="E">` whose
/// text matches `source[S..E]` character for character, as [`render`] writes
/// mapped text.
/// Line endings are outside every span, and the empty line after a final line
/// ending is not rendered.
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
        // Only the last pair has no line ending, and an empty one is what a
        // final line ending leaves, or an empty text.
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
