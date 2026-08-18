//! Reading ticket documents.
//!
//! [`document`] is the entry point: it splits a file into its metadata block,
//! its description, and its comments.
//! Everything here is pure — the caller supplies the text.
//!
//! Comment boundaries are found structurally rather than by looking for the
//! decorative `## Comments` heading: a boundary is a line of five or more
//! dashes at column zero, a blank line, then a metadata block carrying both
//! `From` and `Date`.
//! Lines inside fenced code blocks never count, so a comment quoting a ticket
//! file doesn't split the one it lives in.

use std::ops::Range;

use crate::{Comment, Metadata, ParseError, Ticket};

/// Read a ticket document.
pub fn document(source: &str) -> Result<Ticket, ParseError> {
    let doc = Doc::new(source);
    let title = doc.title()?;
    let header = doc.metadata_range().ok_or(ParseError::MissingMetadata)?;
    let metadata = metadata(header.clone().filter_map(|i| meta_line(doc.lines[i])))?;

    let boundaries = doc.boundaries();
    let description_end = boundaries.first().copied().unwrap_or(doc.lines.len());
    let description = without_comments_heading(doc.text(header.end..description_end));

    let comments = boundaries
        .iter()
        .enumerate()
        .map(|(index, &start)| {
            let end = boundaries
                .get(index + 1)
                .copied()
                .unwrap_or(doc.lines.len());
            doc.comment(start..end)
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Ticket {
        title,
        metadata,
        description,
        comments,
    })
}

/// Count the comments in a ticket document.
///
/// Cheaper than [`document`] and tolerant of a malformed header, so it can run
/// on a file that is about to be appended to rather than validated.
#[must_use]
pub fn comment_count(source: &str) -> usize {
    Doc::new(source).boundaries().len()
}

/// The line range of the metadata block that follows the title heading.
///
/// Returns `None` when the heading is missing, or when the first thing after it
/// isn't a `- **Key**: Value` line.
#[must_use]
pub fn metadata_range(source: &str) -> Option<Range<usize>> {
    Doc::new(source).metadata_range()
}

/// Split a `- **Key**: Value` line into its key and value.
#[must_use]
pub fn meta_line(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.strip_prefix("- **")?.split_once("**:")?;
    Some((key.trim(), value.trim()))
}

/// A line of five or more dashes at column zero: the shape that opens a
/// comment.
fn is_separator(line: &str) -> bool {
    let trimmed = line.trim_end();
    trimmed.len() >= 5 && trimmed.chars().all(|c| c == '-')
}

/// A document, indexed by line, with each line marked as inside or outside a
/// fenced code block.
struct Doc<'a> {
    lines: Vec<&'a str>,
    fenced: Vec<bool>,
}

impl<'a> Doc<'a> {
    fn new(source: &'a str) -> Self {
        let lines: Vec<&str> = source.lines().collect();
        let mut fences = Fences::default();
        let fenced = lines.iter().map(|line| fences.consume(line)).collect();

        Self { lines, fenced }
    }

    /// The title from the `# Title` heading.
    ///
    /// A ticket's id is not in its document — it lives in the filename, so
    /// there is exactly one place that names the ticket and nothing to keep in
    /// step.
    fn title(&self) -> Result<String, ParseError> {
        self.lines
            .iter()
            .find(|line| !line.trim().is_empty())
            .and_then(|line| line.strip_prefix("# "))
            .map(|title| title.trim().to_owned())
            .ok_or(ParseError::MissingTitle)
    }

    fn metadata_range(&self) -> Option<Range<usize>> {
        let title_index = self.lines.iter().position(|line| !line.trim().is_empty())?;
        let start = (title_index + 1..self.lines.len())
            .find(|&i| !self.lines[i].trim().is_empty())
            .filter(|&i| meta_line(self.lines[i]).is_some())?;
        let end = (start..self.lines.len())
            .find(|&i| meta_line(self.lines[i]).is_none())
            .unwrap_or(self.lines.len());

        Some(start..end)
    }

    /// Line indices at which a comment starts.
    fn boundaries(&self) -> Vec<usize> {
        (0..self.lines.len())
            .filter(|&i| !self.fenced[i] && self.is_boundary(i))
            .collect()
    }

    fn is_boundary(&self, index: usize) -> bool {
        if !is_separator(self.lines[index]) {
            return false;
        }
        if !self
            .lines
            .get(index + 1)
            .is_some_and(|line| line.trim().is_empty())
        {
            return false;
        }

        let mut from = false;
        let mut date = false;
        for line in self.lines.iter().skip(index + 2) {
            let Some((key, _)) = meta_line(line) else {
                break;
            };
            from |= key.eq_ignore_ascii_case("from");
            date |= key.eq_ignore_ascii_case("date");
        }

        from && date
    }

    /// Read the comment spanning `range`, which starts at its separator line.
    fn comment(&self, range: Range<usize>) -> Result<Comment, ParseError> {
        let meta_start = (range.start + 2).min(range.end);
        let meta_end = (meta_start..range.end)
            .find(|&i| meta_line(self.lines[i]).is_none())
            .unwrap_or(range.end);

        let mut from = None;
        let mut date = None;
        let mut re = None;
        for (key, value) in (meta_start..meta_end).filter_map(|i| meta_line(self.lines[i])) {
            match key.to_ascii_lowercase().as_str() {
                "from" => from = Some(value.to_owned()),
                "date" => date = Some(value.to_owned()),
                "re" => re = Some(value.to_owned()),
                _ => {}
            }
        }

        Ok(Comment {
            from: from.ok_or(ParseError::MissingField("From"))?,
            date: date.ok_or(ParseError::MissingField("Date"))?,
            re,
            body: self.text(meta_end..range.end),
        })
    }

    fn text(&self, range: Range<usize>) -> String {
        self.lines
            .get(range)
            .unwrap_or_default()
            .join("\n")
            .trim()
            .to_owned()
    }
}

/// Drop the decorative `## Comments` heading a writer leaves above the first
/// comment.
///
/// The heading is not part of the description, and the boundary scan doesn't
/// look at it either — it exists so the file reads well.
fn without_comments_heading(text: String) -> String {
    text.strip_suffix("## Comments")
        .map(|rest| rest.trim_end().to_owned())
        .unwrap_or(text)
}

/// Build a [`Metadata`] from the key/value pairs of a ticket's header.
///
/// Unknown keys are ignored: the file keeps them, and a write only ever
/// replaces the line it targets.
fn metadata<'a>(pairs: impl Iterator<Item = (&'a str, &'a str)>) -> Result<Metadata, ParseError> {
    let mut status = None;
    let mut kind = None;
    let mut authors = None;
    let mut date = None;
    let mut blocked_by = None;
    let mut implements = None;
    let mut promoted_to = None;
    let mut github = None;

    for (key, value) in pairs {
        match key.to_ascii_lowercase().as_str() {
            "status" => status = Some(value.parse()?),
            "kind" => kind = Some(value.parse()?),
            "authors" => authors = Some(value.to_owned()),
            "date" => date = Some(value.to_owned()),
            "blocked by" => blocked_by = Some(value.to_owned()),
            "implements" => implements = Some(value.to_owned()),
            "promoted to" => promoted_to = Some(value.to_owned()),
            "github" => github = Some(value.to_owned()),
            _ => {}
        }
    }

    Ok(Metadata {
        status: status.ok_or(ParseError::MissingField("Status"))?,
        kind: kind.ok_or(ParseError::MissingField("Kind"))?,
        authors: authors.ok_or(ParseError::MissingField("Authors"))?,
        date: date.ok_or(ParseError::MissingField("Date"))?,
        blocked_by,
        implements,
        promoted_to,
        github,
    })
}

/// Tracks fenced code block state while walking a document line by line.
#[derive(Default)]
struct Fences {
    open: Option<(char, usize)>,
}

impl Fences {
    /// Feed the next line, reporting whether it belongs to a fenced code block.
    ///
    /// The opening and closing fences count as inside, so a `-----` sitting
    /// between them is never read as a comment boundary.
    fn consume(&mut self, line: &str) -> bool {
        let Some((char_, count, has_info)) = fence(line) else {
            return self.open.is_some();
        };

        match self.open {
            // A closing fence matches the opener's character, is at least as
            // long, and carries no info string.
            Some((open_char, open_count))
                if char_ == open_char && count >= open_count && !has_info =>
            {
                self.open = None;
                true
            }
            Some(_) => true,
            None => {
                self.open = Some((char_, count));
                true
            }
        }
    }
}

/// Read a line as a code fence delimiter: its character, its run length, and
/// whether an info string follows.
fn fence(line: &str) -> Option<(char, usize, bool)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }

    let char_ = trimmed.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let count = trimmed.chars().take_while(|c| *c == char_).count();
    if count < 3 {
        return None;
    }

    Some((char_, count, !trimmed[count..].trim().is_empty()))
}

#[cfg(test)]
#[path = "parse_tests.rs"]
mod tests;
