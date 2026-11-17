//! The spans rendered text is wrapped in.

use std::{
    fmt::{self, Write},
    ops::Range,
};

use comrak::html::escape;

use crate::range::ESCAPED_PIPE;

/// Write `text[range]` in a mapped span, whose text is the source it covers,
/// character for character.
pub(crate) fn mapped(out: &mut dyn Write, text: &str, range: Range<usize>) -> fmt::Result {
    if range.is_empty() {
        return Ok(());
    }

    write!(
        out,
        "<span data-s=\"{}\" data-e=\"{}\">",
        range.start, range.end
    )?;
    escape(out, &text[range])?;
    out.write_str("</span>")
}

/// Write `shown` in an atom, text that stands for the source `range` as a
/// whole.
pub(crate) fn atom(out: &mut dyn Write, range: Range<usize>, shown: &str) -> fmt::Result {
    write!(
        out,
        "<span data-s=\"{}\" data-e=\"{}\" data-atom>",
        range.start, range.end
    )?;
    escape(out, shown)?;
    out.write_str("</span>")
}

/// Write `shown` in an unmapped span, which claims no source range.
pub(crate) fn unmapped(out: &mut dyn Write, shown: &str) -> fmt::Result {
    if shown.is_empty() {
        return Ok(());
    }

    out.write_str("<span class=\"unmapped\">")?;
    escape(out, shown)?;
    out.write_str("</span>")
}

/// Write `text[range]` in mapped spans, except that each escaped pipe (`\|`) in
/// a table cell is an atom showing `|`.
pub(crate) fn source(
    out: &mut dyn Write,
    text: &str,
    range: Range<usize>,
    in_cell: bool,
) -> fmt::Result {
    let mut from = range.start;
    if in_cell {
        for (index, _) in text[range.clone()].match_indices(ESCAPED_PIPE) {
            let pipe = range.start + index;
            mapped(out, text, from..pipe)?;
            atom(out, pipe..pipe + ESCAPED_PIPE.len(), "|")?;
            from = pipe + ESCAPED_PIPE.len();
        }
    }

    mapped(out, text, from..range.end)
}
