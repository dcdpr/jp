//! Source byte range resolution for AST nodes.
//!
//! Converts the [`Sourcepos`] reported by comrak into UTF-8 byte ranges in the
//! original source string and verifies that the resolved slice matches the
//! rendered text.
//! When verification fails, these functions return `None` so the caller emits
//! an `unmapped` span instead of an inaccurate range.

use std::{borrow::Cow, ops::Range};

use comrak::nodes::{AlertType, LineColumn, NodeCode, Sourcepos};

use crate::position::{Lines, split_lines};

pub(crate) const ESCAPED_PIPE: &str = "\\|";

/// Resolve and verify the source byte range of an inline text or raw HTML node.
///
/// `cell` is the start position of the enclosing table cell, if any.
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

/// Resolve and verify the source byte ranges of a code span, one per source
/// line.
///
/// Multi-line code spans collapse each line ending (and any leading blockquote
/// markers or indentation on the next line) into a single space that has no
/// source range.
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
    // On `\r\n` inputs, comrak retains the `\r` in `code.literal` before the
    // space that replaces the `\n`. Strip `\r` before comparing against the
    // joined source lines.
    let literal = code.literal.replace('\r', "");
    if joined != literal {
        unpad(&mut segments, &joined, &literal)?;
    }

    segments
        .iter()
        .all(|segment| verbatim(&text[segment.clone()]))
        .then_some(segments)
}

/// Resolve the byte range of a single `content` line from a code or HTML block
/// on 1-based source `line`.
///
/// `content` is a suffix of the source line; any bytes preceding it on the line
/// are indentation or blockquote markers.
pub(crate) fn block_line(lines: &Lines<'_>, line: usize, content: &str) -> Option<Range<usize>> {
    let range = lines.line_range(line)?;
    let exact = lines.text()[range.clone()].ends_with(content);

    (exact && verbatim(content)).then(|| range.end - content.len()..range.end)
}

/// Resolve the byte range of an alert marker (such as `[!NOTE]`) on the opening
/// line of the alert at `start`.
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

/// Resolve the byte range of an alert's custom `title` following the `[!KIND]`
/// marker on the opening line at `start`.
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

/// Convert a comrak [`Sourcepos`] to a half-open byte range, adjusting for
/// escaped pipes when inside a table cell starting at `cell`.
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

/// Convert a comrak [`LineColumn`] (1-based line, 1-based byte column) to a
/// byte offset.
fn offset(lines: &Lines<'_>, position: LineColumn) -> Option<usize> {
    let line = lines.line_range(position.line)?;

    Some(line.start + position.column.checked_sub(1)?)
}

/// Convert a comrak [`LineColumn`] inside a table cell to a byte offset.
///
/// comrak counts an escaped pipe (`\|`) in a table cell as one column instead
/// of two bytes, so each `\|` between `cell` and `target` shifts the byte
/// offset forward by one.
/// When `last` is `true` and `target` lands on an escaped pipe, the offset
/// advances to the `|` byte.
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

/// Return the rendered form of `text[range]`, unescaping `\|` to `|` when
/// inside a table cell.
fn shown(text: &str, range: Range<usize>, in_cell: bool) -> Cow<'_, str> {
    let source = &text[range];
    if in_cell && source.contains(ESCAPED_PIPE) {
        Cow::Owned(source.replace(ESCAPED_PIPE, "|"))
    } else {
        Cow::Borrowed(source)
    }
}

/// Split a code span's `content` byte range into per-line ranges, stripping
/// leading blockquote markers and indentation on continuation lines.
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

/// Join the rendered text of a code span's line `segments` with single spaces.
fn join(text: &str, segments: &[Range<usize>], in_cell: bool) -> String {
    segments
        .iter()
        .map(|segment| shown(text, segment.clone(), in_cell))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Trim one leading and one trailing space from `segments` when CommonMark
/// strips symmetric padding from a code span, provided the trimmed text matches
/// `literal`.
///
/// If the first or last segment is empty, the stripped space came from a
/// leading or trailing line ending, so that empty segment is removed outright.
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

/// Strip the leading `>` of a blockquote line at `start`, returning the byte
/// offset and slice of the remainder of the line.
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

/// Return `true` if `text` has the same UTF-8 byte length after HTML parsing.
///
/// comrak replaces `\0` with U+FFFD (3 bytes), and HTML parsers normalize `\r`
/// into `\n`, either of which would skew byte offsets within the span.
fn verbatim(text: &str) -> bool {
    !text.contains(['\0', '\r'])
}
