//! Positions in one version of a file, and how they carry to another version.
//!
//! Lines are counted as `md_html::Lines` counts them: a line ends at `\n`,
//! `\r\n`, or a `\r` not followed by `\n`, and text that ends in a line ending
//! has an empty last line after it.
//! Columns count Unicode code points from 1.
//!
//! A [`Range`] is the part of a file a comment is about, and [`Text`] reads it
//! out of one version of the file.
//! [`LineMap`] carries a range from one version to another through a line diff:
//! the range moves with its lines when every line it covers is unchanged, line
//! endings included, and still adjacent; otherwise it does not carry at all.
//! Nothing here matches text fuzzily.

use std::{fmt, ops, str::FromStr};

use md_html::{LineColumn, Lines};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use similar::{Algorithm, DiffOp, TextDiff, capture_diff_slices};

/// The diff algorithm behind both [`LineMap`] and [`hunks`].
const ALGORITHM: Algorithm = Algorithm::Myers;

/// Unchanged lines a unified diff shows around each change.
const CONTEXT_LINES: usize = 3;

/// The part of a file a comment is about.
///
/// Written `42` or `42-45` for whole lines, and `42:5-43:18` for the text from
/// line 42, column 5, up to but not including line 43, column 18.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    /// Lines `first` through `last`, without the last one's line ending.
    Lines { first: usize, last: usize },

    /// From `start` up to but not including `end`.
    Span { start: LineColumn, end: LineColumn },
}

impl Range {
    /// The first and last line whose text the range covers.
    ///
    /// A span that ends at the start of a later line covers nothing of that
    /// line, only the line ending before it.
    fn covered(self) -> (usize, usize) {
        match self {
            Self::Lines { first, last } => (first, last),
            Self::Span { start, end } if end.column == 1 && end.line > start.line => {
                (start.line, end.line - 1)
            }
            Self::Span { start, end } => (start.line, end.line),
        }
    }

    /// The same range with line `from` moved to line `to`, and every other line
    /// moved with it.
    ///
    /// Every line of the range is at or below `from`.
    fn moved(self, from: usize, to: usize) -> Self {
        let line = |line: usize| line - from + to;

        match self {
            Self::Lines { first, last } => Self::Lines {
                first: line(first),
                last: line(last),
            },
            Self::Span { start, end } => Self::Span {
                start: LineColumn {
                    line: line(start.line),
                    column: start.column,
                },
                end: LineColumn {
                    line: line(end.line),
                    column: end.column,
                },
            },
        }
    }

    fn runs_backwards(self) -> bool {
        match self {
            Self::Lines { first, last } => first > last,
            Self::Span { start, end } => start > end,
        }
    }
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lines { first, last } if first == last => write!(f, "{first}"),
            Self::Lines { first, last } => write!(f, "{first}-{last}"),
            Self::Span { start, end } => write!(
                f,
                "{}:{}-{}:{}",
                start.line, start.column, end.line, end.column
            ),
        }
    }
}

impl FromStr for Range {
    type Err = String;

    /// Read `42`, `42-45`, or `42:5-43:18`.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (from, to) = match text.split_once('-') {
            Some((from, to)) => (from, Some(to)),
            None => (text, None),
        };

        let range = match (position(from), to.map(position)) {
            (Some(Position::Line(line)), None) => Self::Lines {
                first: line,
                last: line,
            },
            (Some(Position::Line(first)), Some(Some(Position::Line(last)))) => {
                Self::Lines { first, last }
            }
            (Some(Position::At(start)), Some(Some(Position::At(end)))) => Self::Span { start, end },
            _ => {
                return Err(format!(
                    "`{text}` is not a range; give a line, `42`, lines, `42-45`, or a span from a \
                     line and column to another, `42:5-43:18`."
                ));
            }
        };

        if range.runs_backwards() {
            return Err(format!("The range `{text}` ends before it starts."));
        }

        Ok(range)
    }
}

impl Serialize for Range {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Range {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

/// One end of a range as written: a line, or a line and a column.
enum Position {
    Line(usize),
    At(LineColumn),
}

fn position(text: &str) -> Option<Position> {
    match text.split_once(':') {
        None => number(text).map(Position::Line),
        Some((line, column)) => Some(Position::At(LineColumn {
            line: number(line)?,
            column: number(column)?,
        })),
    }
}

/// A number from 1 up, written in ASCII digits alone.
fn number(text: &str) -> Option<usize> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }

    text.parse().ok().filter(|number| *number > 0)
}

/// One version of a file, indexed by line.
pub struct Text<'a> {
    text: &'a str,
    lines: Lines<'a>,
}

impl<'a> Text<'a> {
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            lines: Lines::new(text),
        }
    }

    /// Each line, with its line ending.
    ///
    /// The last line has none, and is empty when the text ends in a line
    /// ending.
    pub fn lines(&self) -> Vec<&'a str> {
        let mut lines = vec![];
        let mut number = 1;
        while let Some(line) = self.lines.line_range(number) {
            let end = self
                .lines
                .line_range(number + 1)
                .map_or(self.text.len(), |next| next.start);
            lines.extend(self.text.get(line.start..end));
            number += 1;
        }

        lines
    }

    /// Each line without its line ending; the last is empty when the text ends
    /// in a line ending.
    pub fn contents(&self) -> Vec<&'a str> {
        (1..)
            .map_while(|number| self.lines.line_range(number))
            .filter_map(|range| self.text.get(range))
            .collect()
    }

    /// The bytes `range` covers, or `None` when it reaches outside the text.
    pub fn bytes(&self, range: Range) -> Option<ops::Range<usize>> {
        let (start, end) = match range {
            Range::Lines { first, last } => (
                self.lines.line_range(first)?.start,
                self.lines.line_range(last)?.end,
            ),
            Range::Span { start, end } => (self.lines.offset(start)?, self.lines.offset(end)?),
        };

        (start <= end).then_some(start..end)
    }

    /// The text `range` covers, or `None` when it reaches outside the text.
    pub fn excerpt(&self, range: Range) -> Option<&'a str> {
        self.text.get(self.bytes(range)?)
    }

    /// The span covering `bytes`, UTF-8 byte offsets with the end exclusive.
    ///
    /// `None` when an offset lies outside the text or inside a character, or
    /// the range ends before it starts.
    pub fn span_of(&self, bytes: ops::Range<usize>) -> Option<Range> {
        if bytes.start > bytes.end {
            return None;
        }

        Some(Range::Span {
            start: self.lines.line_column(bytes.start)?,
            end: self.lines.line_column(bytes.end)?,
        })
    }

    /// Return each line with its line ending, omitting the trailing empty
    /// segment after a final newline.
    pub fn shown(&self) -> Vec<&'a str> {
        let mut lines = self.lines();
        if lines.last().is_some_and(|line| line.is_empty()) {
            lines.pop();
        }

        lines
    }

    /// Narrow `lines` to the one place in them that holds `quote`.
    ///
    /// Refused when `lines` reach outside the text, or `quote` is empty or
    /// occurs in them other than once, overlapping occurrences included.
    pub fn narrow(&self, lines: Range, quote: &str) -> Result<Range, String> {
        if quote.is_empty() {
            return Err("`quote` is empty.".to_owned());
        }
        let bytes = self
            .bytes(lines)
            .ok_or_else(|| format!("Lines {lines} lie outside the file."))?;
        let within = self.text.get(bytes.clone()).unwrap_or_default();

        let start = match occurrences(within, quote).as_slice() {
            [] => return Err(format!("`quote` doesn't occur in lines {lines}.")),
            [only] => bytes.start + only,
            many => {
                return Err(format!(
                    "`quote` occurs {} times in lines {lines}; quote more of the line to name one \
                     of them.",
                    many.len()
                ));
            }
        };
        let position = |offset| self.lines.line_column(offset);

        match (position(start), position(start + quote.len())) {
            (Some(start), Some(end)) => Ok(Range::Span { start, end }),
            _ => Err(format!("`quote` doesn't occur in lines {lines}.")),
        }
    }
}

/// Where `needle` starts in `haystack`, each time it occurs, overlapping
/// occurrences included; `needle` is not empty.
fn occurrences(haystack: &str, needle: &str) -> Vec<usize> {
    let step = needle.chars().next().map_or(1, char::len_utf8);
    let mut found = vec![];
    let mut from = 0;
    while let Some(index) = haystack.get(from..).and_then(|rest| rest.find(needle)) {
        found.push(from + index);
        from += index + step;
    }

    found
}

/// How the lines of one version of a file map onto another's.
pub struct LineMap {
    ops: Vec<DiffOp>,
}

impl LineMap {
    /// Diff `old` against `new` by line, without their line endings: a file
    /// converted between `\r\n` and `\n` keeps every line, and the columns a
    /// range counts within one.
    pub fn new(old: &Text<'_>, new: &Text<'_>) -> Self {
        Self {
            ops: capture_diff_slices(ALGORITHM, &old.contents(), &new.contents()),
        }
    }

    /// Where `range` in the old version falls in the new one.
    ///
    /// `None` when a line the range covers changed, or when the lines it covers
    /// are no longer adjacent.
    pub fn project(&self, range: Range) -> Option<Range> {
        let (first, last) = range.covered();
        let to = self.ops.iter().find_map(|op| kept(op, first, last))?;

        Some(range.moved(first, to))
    }
}

/// The line `first` lands on when `op` keeps lines `first` through `last`
/// unchanged and together.
fn kept(op: &DiffOp, first: usize, last: usize) -> Option<usize> {
    let DiffOp::Equal {
        old_index,
        new_index,
        len,
    } = *op
    else {
        return None;
    };

    // `DiffOp` counts lines from 0.
    let (start, end) = (first.checked_sub(1)?, last.checked_sub(1)?);
    if start < old_index || end >= old_index + len {
        return None;
    }

    Some(new_index + (start - old_index) + 1)
}

/// Where one version of a file differs from an earlier one, by line, in byte
/// offsets into the later version.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Each line added or modified, without its line ending, as its half-open
    /// byte range `[start, end)`.
    pub added: Vec<[usize; 2]>,

    /// Where each run of removed lines was: the start of the line after it, or
    /// the end of the text.
    pub removed: Vec<usize>,
}

/// Where `new` differs from `old`, compared line by line as [`hunks`] compares
/// them: a line whose line ending changed is modified.
///
/// Lines that replace removed ones count as modified, and only a run of removed
/// lines with nothing in its place counts as removed.
pub fn changes(old: &str, new: &str) -> Changes {
    let (before, after) = (Text::new(old), Text::new(new));
    // `DiffOp` counts lines from 0, and `Lines` from 1.
    let start = |index: usize| {
        after
            .lines
            .line_range(index + 1)
            .map_or(new.len(), |line| line.start)
    };

    let mut changes = Changes::default();
    for op in capture_diff_slices(ALGORITHM, &before.shown(), &after.shown()) {
        match op {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete { new_index, .. } => changes.removed.push(start(new_index)),
            DiffOp::Insert {
                new_index, new_len, ..
            }
            | DiffOp::Replace {
                new_index, new_len, ..
            } => changes.added.extend(
                (new_index + 1..=new_index + new_len)
                    .filter_map(|number| after.lines.line_range(number))
                    .map(|line| [line.start, line.end]),
            ),
        }
    }

    changes
}

/// The unified diff hunks from `old` to `new`, with three lines of context.
///
/// Empty when the two hold the same lines.
/// A last line without a line ending is marked `\ No newline at end of file`.
pub fn hunks(old: &str, new: &str) -> String {
    let (old, new) = (Text::new(old).shown(), Text::new(new).shown());

    TextDiff::configure()
        .algorithm(ALGORITHM)
        .newline_terminated(true)
        .diff_slices(&old, &new)
        .unified_diff()
        .context_radius(CONTEXT_LINES)
        .to_string()
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
