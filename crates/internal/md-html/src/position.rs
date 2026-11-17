//! Conversions between byte offsets into a text and 1-based line and column
//! positions.

use std::ops::Range;

/// A 1-based line and column in a text.
///
/// The column counts Unicode code points from the start of the line: the first
/// character on a line is at column 1, whatever its size in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LineColumn {
    pub line: usize,
    pub column: usize,
}

/// The lines of a text, for converting between byte offsets into it and
/// [`LineColumn`] positions.
///
/// A line ends at `\n`, `\r\n`, or a `\r` not followed by `\n`, as in
/// CommonMark.
/// The characters of a line ending belong to the line they end.
#[derive(Debug, Clone)]
pub struct Lines<'s> {
    text: &'s str,

    /// The bytes of each line, without its line ending.
    ranges: Vec<Range<usize>>,
}

impl<'s> Lines<'s> {
    /// Find where each line of `text` starts and ends.
    #[must_use]
    pub fn new(text: &'s str) -> Self {
        let mut ranges = vec![];
        let mut start = 0;
        for (line, ending) in split_lines(text) {
            ranges.push(start..start + line.len());
            start += line.len() + ending.len();
        }

        Self { text, ranges }
    }

    /// The position of byte `offset`.
    ///
    /// `offset` may be the size of the text, the position past its last
    /// character.
    /// Returns `None` for an offset past that, or one inside a character.
    // No code in the workspace calls this yet (T-1md6ywd).
    // qual:api
    #[must_use]
    pub fn line_column(&self, offset: usize) -> Option<LineColumn> {
        let before = self.text.get(..offset)?;
        let index = self
            .ranges
            .partition_point(|range| range.start <= offset)
            .checked_sub(1)?;
        let start = self.ranges.get(index)?.start;
        let column = before.get(start..)?.chars().count() + 1;

        Some(LineColumn {
            line: index + 1,
            column,
        })
    }

    /// The byte offset of `position`.
    ///
    /// A line's columns run through its line ending, and the last line also has
    /// the column past its last character, at the end of the text.
    /// Returns `None` for any other position.
    // No code in the workspace calls this yet (T-1md6ywd).
    // qual:api
    #[must_use]
    pub fn offset(&self, position: LineColumn) -> Option<usize> {
        let index = position.line.checked_sub(1)?;
        let start = self.ranges.get(index)?.start;
        let next = self.ranges.get(index + 1).map(|next| next.start);
        let line = self.text.get(start..next.unwrap_or(self.text.len()))?;
        let offset = line
            .char_indices()
            .map(|(offset, _)| offset)
            .chain(next.is_none().then_some(line.len()))
            .nth(position.column.checked_sub(1)?)?;

        Some(start + offset)
    }

    pub(crate) fn text(&self) -> &'s str {
        self.text
    }

    /// The bytes of 1-based line `line`, without its line ending.
    pub(crate) fn line_range(&self, line: usize) -> Option<Range<usize>> {
        self.ranges.get(line.checked_sub(1)?).cloned()
    }
}

/// Split `text` at each line ending CommonMark reads: `\n`, `\r\n`, or a `\r`
/// not followed by `\n`.
///
/// Returns each line with the line ending after it, which is empty for the last
/// line.
pub(crate) fn split_lines(text: &str) -> Vec<(&str, &str)> {
    let mut lines = vec![];
    let mut rest = text;
    while let Some(index) = rest.find(['\n', '\r']) {
        let size = if rest[index..].starts_with("\r\n") {
            2
        } else {
            1
        };
        lines.push((&rest[..index], &rest[index..index + size]));
        rest = &rest[index + size..];
    }
    lines.push((rest, ""));

    lines
}

#[cfg(test)]
#[path = "position_tests.rs"]
mod tests;
