//! The `- **Key**: Value` lines a document carries under its title, one per
//! field.
//!
//! ```markdown
//! # Title
//!
//! - **Status**: Accepted
//! - **Label**: domain=conversation
//! - **Summary**: One sentence, long enough to wrap onto
//!   an indented line.
//!
//! ## First section
//! ```
//!
//! The block runs from the first field line to the blank line that closes it; a
//! section heading (`## Summary`) before any field line means the document has
//! none.
//! A value long enough to wrap continues on the lines below its field, as
//! `comfort` lays it out at 80 columns, up to the next field.
//! Keys match without case, and a field can repeat, one line per value, as
//! `Label` does.
//! The site reads the block the same way, in `jp-vite`'s `metadata.mjs`.
//!
//! [`split`] reads one field line and [`line()`] writes one.
//! [`Header`] reads a block and nothing past it, so a field line quoted further
//! down, as a document describing the format does, is never read as the
//! document's own.
//! [`Document`] edits one, leaving every line outside the fields it changes as
//! it was.

use std::ops::Range;

/// Split a `- **Key**: Value` line into its key and value, both trimmed.
///
/// `None` for any other line, an indented one included.
#[must_use]
pub fn split(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.strip_prefix("- **")?.split_once("**:")?;

    Some((key.trim(), value.trim()))
}

/// The field line `- **key**: value`.
///
/// The value is written on one line, however long; `comfort` wraps it where the
/// document is reflowed.
#[must_use]
pub fn line(key: &str, value: &str) -> String {
    format!("- **{key}**: {value}")
}

/// Undo the escaping a markdown formatter applies inside a value.
///
/// The block is a markdown list, so a formatter escapes what would otherwise be
/// emphasis or a heading: `package=jp_config` is written `package=jp\_config`.
/// Reading has to undo it, or a value round-trips into something else.
#[must_use]
pub fn unescape(value: &str) -> String {
    if !value.contains('\\') {
        return value.to_owned();
    }

    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(char_) = chars.next() {
        if char_ != '\\' {
            out.push(char_);
            continue;
        }

        // A backslash escapes ASCII punctuation and nothing else, so one before
        // anything is itself.
        match chars.next() {
            Some(next) if next.is_ascii_punctuation() => out.push(next),
            Some(next) => {
                out.push('\\');
                out.push(next);
            }
            None => out.push('\\'),
        }
    }

    out
}

/// The metadata block of `lines`: the run of lines from the first `- **` line
/// to the blank line that closes it.
///
/// A section heading (`## Summary`) before any field line means the document
/// has no block, which is an empty range.
#[must_use]
pub fn block<S: AsRef<str>>(lines: &[S]) -> Range<usize> {
    let mut start = None;
    for (index, line) in lines.iter().enumerate() {
        let line = line.as_ref();
        if line.starts_with("## ") {
            break;
        }
        if line.starts_with("- **") {
            start = Some(index);
            break;
        }
    }

    let Some(start) = start else {
        return 0..0;
    };

    let end = lines[start..]
        .iter()
        .position(|line| line.as_ref().trim().is_empty())
        .map_or(lines.len(), |offset| start + offset);

    start..end
}

/// The lines the field `key` spans in `lines`: its own line, and the lines a
/// value long enough to wrap continues on, up to the next field.
///
/// `comfort` indents a continuation past the list marker; a line at the margin
/// continues the value too, as markdown reads a list item.
fn field_lines<S: AsRef<str>>(lines: &[S], key: &str) -> Option<Range<usize>> {
    let block = block(lines);
    let first = block
        .clone()
        .find(|&index| names(lines[index].as_ref(), key))?;

    Some(first..field_end(lines, first, block.end))
}

/// Where the field opening at `first` ends: the next line that opens a field,
/// or `end`, the end of the block.
fn field_end<S: AsRef<str>>(lines: &[S], first: usize, end: usize) -> usize {
    (first + 1..end)
        .find(|&index| lines[index].as_ref().starts_with("- **"))
        .unwrap_or(end)
}

/// Whether `line` opens the field `key`, matched without case.
fn names(line: &str, key: &str) -> bool {
    split(line).is_some_and(|(found, _)| found.eq_ignore_ascii_case(key))
}

/// One field of a block, as [`Header::fields`] reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// The key as the line writes it.
    pub key: String,

    /// The value with the lines it wraps onto, joined with single spaces; empty
    /// for a field written without one.
    pub value: String,

    /// The lines the field spans: its own, and its continuation lines.
    pub lines: Range<usize>,
}

/// A document's metadata block, read.
pub struct Header<'a, S> {
    lines: &'a [S],
}

impl<'a, S: AsRef<str>> Header<'a, S> {
    /// The metadata of the document `lines` holds.
    #[must_use]
    pub fn of(lines: &'a [S]) -> Self {
        Self { lines }
    }

    /// Every line of the document, the block and what surrounds it.
    #[must_use]
    pub fn lines(&self) -> &'a [S] {
        self.lines
    }

    /// The first line's value of the field `key`, trimmed.
    ///
    /// `None` when the block has no such field, or it is empty.
    #[must_use]
    pub fn field(&self, key: &str) -> Option<String> {
        let range = field_lines(self.lines, key)?;
        let (_, value) = split(self.lines[range.start].as_ref())?;

        (!value.is_empty()).then(|| value.to_owned())
    }

    /// The value of the field `key` with the lines it wraps onto, as one line.
    ///
    /// A value long enough to wrap continues on the lines below it, up to the
    /// next field; those are joined back with single spaces.
    #[must_use]
    pub fn wrapped(&self, key: &str) -> Option<String> {
        let range = field_lines(self.lines, key)?;
        let value = joined(self.lines, range);

        (!value.is_empty()).then_some(value)
    }

    /// Every field of the block, in order, a repeated one once per line that
    /// opens it.
    ///
    /// A `- **` line that isn't a field, such as one missing its `**:`, opens
    /// nothing, and the lines continuing it are passed over.
    #[must_use]
    pub fn fields(&self) -> Vec<Field> {
        let block = block(self.lines);

        block
            .clone()
            .filter_map(|index| {
                let (key, _) = split(self.lines[index].as_ref())?;
                let lines = index..field_end(self.lines, index, block.end);

                Some(Field {
                    key: key.to_owned(),
                    value: joined(self.lines, lines.clone()),
                    lines,
                })
            })
            .collect()
    }

    /// The value of every line that opens the field `key`, in order, wrapped
    /// lines joined; empty values included.
    #[must_use]
    pub fn values(&self, key: &str) -> Vec<String> {
        self.fields()
            .into_iter()
            .filter(|field| field.key.eq_ignore_ascii_case(key))
            .map(|field| field.value)
            .collect()
    }

    /// Whether the line at `index` belongs to the field `key`, continuation
    /// lines included.
    #[must_use]
    pub fn is_field_line(&self, key: &str, index: usize) -> bool {
        field_lines(self.lines, key).is_some_and(|range| range.contains(&index))
    }
}

/// The value of the field spanning `range`: what follows its key, and its
/// continuation lines, with each run of whitespace read as one space.
fn joined<S: AsRef<str>>(lines: &[S], range: Range<usize>) -> String {
    let mut value = split(lines[range.start].as_ref())
        .map(|(_, value)| value.to_owned())
        .unwrap_or_default();
    for line in &lines[range.start + 1..range.end] {
        value.push(' ');
        value.push_str(line.as_ref());
    }

    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A document without a metadata block, asked to change one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoMetadata;

/// A document being edited.
///
/// Edits touch the fields they name, and leave every other line as it was.
/// A file read with `\r\n` line endings is written back with them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    lines: Vec<String>,
    crlf: bool,
    trailing_newline: bool,
}

impl Document {
    /// Read `text`, keeping its line endings and whether it ends in one.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        Self {
            lines: text.lines().map(ToOwned::to_owned).collect(),
            crlf: text.contains("\r\n"),
            trailing_newline: text.ends_with('\n'),
        }
    }

    /// The document as text, with the line endings it was read with.
    #[must_use]
    pub fn render(&self) -> String {
        let mut text = self.lines.join(if self.crlf { "\r\n" } else { "\n" });
        if self.trailing_newline {
            text.push_str(if self.crlf { "\r\n" } else { "\n" });
        }

        text
    }

    /// Every line, without its line ending.
    #[must_use]
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// The lines, for an edit to a line outside the block, such as the title
    /// heading, that no other method makes.
    ///
    /// The line count is fixed, so the document keeps its line endings.
    pub fn lines_mut(&mut self) -> &mut [String] {
        &mut self.lines
    }

    /// The metadata block, read.
    #[must_use]
    pub fn header(&self) -> Header<'_, String> {
        Header::of(&self.lines)
    }

    /// Where a field the block doesn't carry yet goes: above `Summary`, which
    /// the block keeps last, or at the end of the block when there is none.
    fn insertion_point(&self) -> Result<usize, NoMetadata> {
        let block = block(&self.lines);
        if block.is_empty() {
            return Err(NoMetadata);
        }

        Ok(field_lines(&self.lines, "Summary").map_or(block.end, |summary| summary.start))
    }

    /// Set the field `key` to `value`, replacing it whole, wrapped lines
    /// included, or adding it when the block doesn't carry it.
    ///
    /// # Errors
    ///
    /// Returns [`NoMetadata`] when the field is missing and there is no block
    /// to add it to.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), NoMetadata> {
        let field = line(key, value);

        if let Some(range) = field_lines(&self.lines, key) {
            self.lines.splice(range, [field]);
            return Ok(());
        }

        let at = self.insertion_point()?;
        self.lines.insert(at, field);

        Ok(())
    }

    /// Replace every line of the field `key`, wrapped lines included, with one
    /// line per value.
    ///
    /// The new lines take the place of the first old one, so the field keeps
    /// its place in the block; a field the block doesn't carry joins it where
    /// [`Document::set`] adds one.
    /// No values removes the field.
    ///
    /// # Errors
    ///
    /// Returns [`NoMetadata`] when the document has no block.
    pub fn set_repeated(&mut self, key: &str, values: &[String]) -> Result<(), NoMetadata> {
        if block(&self.lines).is_empty() {
            return Err(NoMetadata);
        }

        let spans: Vec<Range<usize>> = self
            .header()
            .fields()
            .into_iter()
            .filter(|field| field.key.eq_ignore_ascii_case(key))
            .map(|field| field.lines)
            .collect();
        let at = match spans.first() {
            Some(first) => first.start,
            None => self.insertion_point()?,
        };

        // From the last, so the ranges still to remove keep their places.
        for span in spans.into_iter().rev() {
            self.lines.drain(span);
        }
        self.lines
            .splice(at..at, values.iter().map(|value| line(key, value)));

        Ok(())
    }

    /// Remove the field `key`, wrapped lines included.
    /// `false` when the block doesn't carry it.
    pub fn remove(&mut self, key: &str) -> bool {
        let Some(range) = field_lines(&self.lines, key) else {
            return false;
        };

        self.lines.drain(range);
        true
    }

    /// Add `text` to the end of the value of the field `key`: after the last
    /// line the value wraps onto, with that line's trailing whitespace dropped.
    ///
    /// `false` when the block doesn't carry the field, which leaves the
    /// document as it was.
    pub fn append(&mut self, key: &str, text: &str) -> bool {
        let Some(range) = field_lines(&self.lines, key) else {
            return false;
        };

        let last = &mut self.lines[range.end - 1];
        let kept = last.trim_end().len();
        last.truncate(kept);
        last.push_str(text);

        true
    }

    /// Insert `notice` between the metadata and the first section, followed by
    /// a blank line.
    ///
    /// A document without sections gets it at the end.
    pub fn insert_before_sections(&mut self, notice: &[String]) {
        let after_metadata = block(&self.lines).end;
        let section =
            (after_metadata..self.lines.len()).find(|&index| self.lines[index].starts_with("## "));

        if let Some(at) = section {
            let inserted = notice.iter().cloned().chain([String::new()]);
            self.lines.splice(at..at, inserted);
            return;
        }

        if self
            .lines
            .last()
            .is_some_and(|line| !line.trim().is_empty())
        {
            self.lines.push(String::new());
        }
        self.lines.extend(notice.iter().cloned());
        self.trailing_newline = true;
    }
}

#[cfg(test)]
#[path = "metadata_tests.rs"]
mod tests;
