//! Conversion between UTF-8 byte offsets and 1-based line and column positions.

use std::ops::Range;

/// A 1-based line and column position in a string.
///
/// `column` counts Unicode scalar values (`char`s) from the start of the line,
/// so the first character on a line is at column 1 regardless of its UTF-8 byte
/// length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LineColumn {
    pub line: usize,
    pub column: usize,
}

/// Line index over a source string for converting between UTF-8 byte offsets
/// and [`LineColumn`] positions.
///
/// Follows CommonMark line endings: `\n`, `\r\n`, or a standalone `\r`.
/// Line ending characters belong to the line they terminate.
#[derive(Debug, Clone)]
pub struct Lines<'s> {
    text: &'s str,

    /// Byte range of each line, excluding its line ending.
    ranges: Vec<Range<usize>>,
}

impl<'s> Lines<'s> {
    /// Index the line boundaries of `text`.
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

    /// Convert a UTF-8 byte `offset` to a 1-based [`LineColumn`].
    ///
    /// `offset == text.len()` resolves to the position immediately after the
    /// last character.
    /// Returns `None` if `offset` is out of bounds or does not lie on a UTF-8
    /// character boundary.
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

    /// Convert a 1-based [`LineColumn`] to a UTF-8 byte offset.
    ///
    /// Valid columns on a line include its line ending characters, and on the
    /// last line also include the position one past the final character
    /// (`text.len()`).
    /// Returns `None` if `position` is out of bounds.
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

    /// Return the byte range of 1-based `line`, excluding its line ending.
    pub(crate) fn line_range(&self, line: usize) -> Option<Range<usize>> {
        self.ranges.get(line.checked_sub(1)?).cloned()
    }
}

/// Split `text` on CommonMark line endings (`\n`, `\r\n`, or standalone `\r`).
///
/// Yields `(line_without_ending, ending)` pairs, ending with a final pair whose
/// `ending` is `""`.
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
