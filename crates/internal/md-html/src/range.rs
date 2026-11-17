//! The source ranges of rendered text.
//!
//! comrak reports a source position for each node.
//! These functions turn a node's position into the byte ranges its rendered
//! text comes from, and return `None` when those bytes are not the rendered
//! text, so the text is rendered unmapped instead of with a range that is
//! wrong.

use std::{borrow::Cow, ops::Range};

use comrak::nodes::{AlertType, LineColumn, NodeCode, Sourcepos};

use crate::position::{Lines, split_lines};

/// The escape that puts a `|` in a table cell.
pub(crate) const ESCAPED_PIPE: &str = "\\|";

/// The range of a text or raw HTML node's `literal`.
///
/// `cell` is the start of the table cell the node is in, if any.
pub(crate) fn inline(
    lines: &Lines<'_>,
    pos: Sourcepos,
    cell: Option<LineColumn>,
    literal: &str,
) -> Option<Range<usize>> {
    let range = node(lines, pos, cell)?;
    let exact = shown(lines.text(), range.clone(), cell.is_some()) == literal;

    (exact && verbatim(literal)).then_some(range)
}

/// The ranges a code span's content comes from, one for each source line it
/// spans.
///
/// The code span renders each line ending, with the container markers and
/// indentation that start the next line, as one space, which is in no range.
pub(crate) fn code_span(
    lines: &Lines<'_>,
    pos: Sourcepos,
    cell: Option<LineColumn>,
    code: &NodeCode,
) -> Option<Vec<Range<usize>>> {
    let text = lines.text();
    let range = node(lines, pos, cell)?;
    let fence = "`".repeat(code.num_backticks);
    let content = text[range.clone()]
        .strip_prefix(fence.as_str())?
        .strip_suffix(fence.as_str())?;
    let start = range.start + fence.len();

    let mut segments = code_lines(text, start..start + content.len());
    let joined = join(text, &segments, cell.is_some());
    // comrak keeps the `\r` of a `\r\n` line ending in the literal, before the
    // space that stands for the line ending. A lone `\r` becomes the space.
    let literal = code.literal.replace('\r', "");
    if joined != literal {
        unpad(&mut segments, &joined, &literal)?;
    }

    segments
        .iter()
        .all(|segment| verbatim(&text[segment.clone()]))
        .then_some(segments)
}

/// The range of `content`, a line of a code or HTML block, on source line
/// `line`.
///
/// The content ends its source line: what comes before it on the line is
/// indentation and container markers.
pub(crate) fn block_line(lines: &Lines<'_>, line: usize, content: &str) -> Option<Range<usize>> {
    let range = lines.line_range(line)?;
    let exact = lines.text()[range.clone()].ends_with(content);

    (exact && verbatim(content)).then(|| range.end - content.len()..range.end)
}

/// The range of an alert's marker, such as `[!NOTE]`, on the line where the
/// alert starts at `start`.
pub(crate) fn alert_marker(
    lines: &Lines<'_>,
    start: LineColumn,
    kind: AlertType,
) -> Option<Range<usize>> {
    let (offset, rest) = after_quote(lines, start)?;
    let marker = rest.trim_start_matches([' ', '\t']);
    let open = offset + (rest.len() - marker.len());
    let name = marker.strip_prefix("[!")?;
    let size = name.find(']')?;

    name[..size]
        .eq_ignore_ascii_case(kind.default_title())
        .then(|| open..open + "[!".len() + size + "]".len())
}

/// The range of an alert's own `title`: the rest of the line after its marker.
pub(crate) fn alert_title(
    lines: &Lines<'_>,
    start: LineColumn,
    title: &str,
) -> Option<Range<usize>> {
    let (offset, rest) = after_quote(lines, start)?;
    let after = &rest[rest.find(']')? + "]".len()..];
    let trimmed = after.trim_start_matches(is_title_space);
    let from = offset + (rest.len() - trimmed.len());
    let exact = trimmed.trim_end_matches(is_title_space) == title;

    (exact && verbatim(title)).then(|| from..from + title.len())
}

/// The bytes `pos` covers, for a node in the table cell starting at `cell`, if
/// any.
fn node(lines: &Lines<'_>, pos: Sourcepos, cell: Option<LineColumn>) -> Option<Range<usize>> {
    let (start, last) = match cell {
        Some(cell) => (
            cell_offset(lines, cell, pos.start, false)?,
            cell_offset(lines, cell, pos.end, true)?,
        ),
        None => (offset(lines, pos.start)?, offset(lines, pos.end)?),
    };
    let range = start..last + 1;
    lines.text().get(range.clone())?;

    Some(range)
}

/// The byte offset of a comrak position: a 1-based line, and a 1-based column
/// that counts bytes.
fn offset(lines: &Lines<'_>, position: LineColumn) -> Option<usize> {
    let line = lines.line_range(position.line)?;

    Some(line.start + position.column.checked_sub(1)?)
}

/// The byte offset of `target`, a position in the table cell starting at
/// `cell`.
///
/// comrak counts an escaped pipe (`\|`) in a cell as one column, so each one
/// before `target` in the cell puts it a byte further along.
/// `last` marks `target` as a node's last byte, which for an escaped pipe is
/// the `|`.
fn cell_offset(
    lines: &Lines<'_>,
    cell: LineColumn,
    target: LineColumn,
    last: bool,
) -> Option<usize> {
    if target.line != cell.line || target.column < cell.column {
        return offset(lines, target);
    }

    let bytes = lines.text().as_bytes();
    let escaped_pipe = |at: usize| {
        bytes
            .get(at..)
            .is_some_and(|rest| rest.starts_with(ESCAPED_PIPE.as_bytes()))
    };

    let mut at = offset(lines, cell)?;
    for _ in cell.column..target.column {
        at += if escaped_pipe(at) {
            ESCAPED_PIPE.len()
        } else {
            1
        };
    }

    Some(if last && escaped_pipe(at) { at + 1 } else { at })
}

/// The text `text[range]` renders as: in a table cell, an escaped pipe renders
/// as `|`.
fn shown(text: &str, range: Range<usize>, in_cell: bool) -> Cow<'_, str> {
    let source = &text[range];
    if in_cell && source.contains(ESCAPED_PIPE) {
        Cow::Owned(source.replace(ESCAPED_PIPE, "|"))
    } else {
        Cow::Borrowed(source)
    }
}

/// The range of each line of a code span's `content`, past the container
/// markers and indentation that start each line after the first.
fn code_lines(text: &str, content: Range<usize>) -> Vec<Range<usize>> {
    let mut segments = vec![];
    let mut start = content.start;
    for (index, (line, ending)) in split_lines(&text[content]).into_iter().enumerate() {
        let prefix = if index == 0 {
            0
        } else {
            line.len() - line.trim_start_matches([' ', '\t', '>']).len()
        };
        segments.push(start + prefix..start + line.len());
        start += line.len() + ending.len();
    }

    segments
}

/// The text code span `segments` render as, a space standing for each line
/// ending between them.
fn join(text: &str, segments: &[Range<usize>], in_cell: bool) -> String {
    segments
        .iter()
        .map(|segment| shown(text, segment.clone(), in_cell))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Drop the space CommonMark strips from each end of a code span's content when
/// it starts and ends with one, if that leaves `literal`.
///
/// An empty first or last segment is a line ending at that end of the content,
/// which is the space, so the segment is dropped whole.
fn unpad(segments: &mut Vec<Range<usize>>, joined: &str, literal: &str) -> Option<()> {
    let inner = joined.strip_prefix(' ')?.strip_suffix(' ')?;
    if inner != literal {
        return None;
    }

    if segments.first()?.is_empty() {
        segments.remove(0);
    } else {
        segments.first_mut()?.start += 1;
    }

    if segments.last()?.is_empty() {
        segments.pop();
    } else {
        segments.last_mut()?.end -= 1;
    }

    Some(())
}

/// The rest of the line after the `>` at `start`, and the offset the rest
/// starts at.
fn after_quote<'s>(lines: &Lines<'s>, start: LineColumn) -> Option<(usize, &'s str)> {
    let quote = offset(lines, start)?;
    let line = lines.line_range(start.line)?;
    let rest = lines.text().get(quote..line.end)?.strip_prefix('>')?;

    Some((quote + ">".len(), rest))
}

/// Whitespace comrak trims from an alert's title.
fn is_title_space(char_: char) -> bool {
    matches!(char_, ' ' | '\t' | '\u{0b}' | '\u{0c}')
}

/// Whether a page shows `text` with the bytes it has in the source.
///
/// comrak writes `\0` as U+FFFD, and an HTML parser reads `\r\n` as `\n`, so
/// either would shift the offsets of the text after it.
fn verbatim(text: &str) -> bool {
    !text.contains(['\0', '\r'])
}
