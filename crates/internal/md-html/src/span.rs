//! HTML `<span>` emitters for source-mapped, atom, and unmapped text.

use std::{
    fmt::{self, Write},
    ops::Range,
};

use comrak::html::escape;

use crate::range::ESCAPED_PIPE;

/// Write `text[range]` as a `<span data-s=".." data-e="..">` whose text matches
/// the source slice character for character.
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

/// Write `shown` as a `<span data-s=".." data-e=".." data-atom>` representing
/// `range` as an indivisible unit.
pub(crate) fn atom(out: &mut dyn Write, range: Range<usize>, shown: &str) -> fmt::Result {
    write!(
        out,
        "<span data-s=\"{}\" data-e=\"{}\" data-atom>",
        range.start, range.end
    )?;
    escape(out, shown)?;
    out.write_str("</span>")
}

/// Write `shown` as `<span class="unmapped">` without source offsets.
pub(crate) fn unmapped(out: &mut dyn Write, shown: &str) -> fmt::Result {
    if shown.is_empty() {
        return Ok(());
    }

    out.write_str("<span class=\"unmapped\">")?;
    escape(out, shown)?;
    out.write_str("</span>")
}

/// Write `text[range]` as mapped spans, splitting out any escaped pipe (`\|`)
/// in a table cell as a `data-atom` span displaying `|`.
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
