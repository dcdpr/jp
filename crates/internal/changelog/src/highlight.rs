//! One highlight: an editorial section of the release notes, for a feature
//! worth more than its entry.
//!
//! A highlight is a Markdown file in a `highlights/` directory, named
//! `<slug>.md`.
//! It opens with the section's title, then lists the people it credits in a
//! metadata block, one `Authors` field each, and the rest is the section's
//! prose:
//!
//! ```markdown
//! # Unified editor and inline reply widget
//!
//! - **Authors**: @JeanMertz
//!
//! Every prompt in JP now runs through a single editor service, and short
//! replies can be written inline without leaving the conversation view.
//! ```
//!
//! The block can list `Credits` too, which replace the credits history names,
//! as a fragment's do.
//!
//! A highlight has no type and no upgrade section: the fragment for the change
//! carries both.
//! A line that starts with an upgrade section's token is an error, so a section
//! meant for the upgrade guide can't end up in the release notes' prose
//! instead.

use std::fmt;

use crate::{
    document::{self, CREDITS, ListedCredit},
    links::{Links, line_starts},
};

/// The field that names someone to credit for the feature.
const AUTHORS: &str = "Authors";

/// A highlight, as [`parse`] reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlight {
    /// The heading of the highlight's section in the release notes.
    pub title: String,

    /// The people credited for the feature, as written, such as `@JeanMertz`.
    pub authors: Vec<String>,

    /// The credits to give in place of the ones history names, when the file
    /// lists them.
    pub credits: Option<Vec<ListedCredit>>,

    /// The section's prose, as Markdown.
    ///
    /// Its links are inline, `[text](url)`, whichever form the file wrote them
    /// in, so the prose stands alone beside other highlights.
    pub body: String,
}

/// A document that doesn't hold a well-formed highlight.
///
/// Lines count from 1 at the top of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HighlightError {
    /// The first line with text isn't a `# Title` heading with a title.
    NoTitle,

    /// The metadata block lists no `Authors`.
    NoAuthors,

    /// An `Authors` field names nobody.
    EmptyAuthor { line: usize },

    /// The metadata block lists a field other than `Authors` and `Credits`.
    UnknownField { line: usize, key: String },

    /// A `Credits` field names neither a change request nor a commit.
    InvalidCredit { line: usize, value: String },

    /// Nothing but whitespace follows the title and the metadata block.
    EmptyBody,

    /// A line starts with an upgrade section's token.
    Marker { line: usize, marker: &'static str },
}

impl fmt::Display for HighlightError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoTitle => f.write_str(
                "no title: a highlight opens with `# Title`, which heads its section of the \
                 release notes",
            ),
            Self::NoAuthors => write!(
                f,
                "no authors: list each person to credit for the feature below the title, as `- \
                 **{AUTHORS}**: @JeanMertz`"
            ),
            Self::EmptyAuthor { line } => write!(
                f,
                "line {line} credits nobody: write who to credit after `- **{AUTHORS}**:`"
            ),
            Self::UnknownField { line, key } => write!(
                f,
                "line {line} lists `{key}`, and the fields a highlight takes are `{AUTHORS}` and \
                 `{CREDITS}`"
            ),
            Self::InvalidCredit { line, value } => {
                f.write_str(&document::invalid_credit(*line, value))
            }
            Self::EmptyBody => f.write_str(
                "no prose: write what the feature does for users below the title and its authors",
            ),
            Self::Marker { line, marker } => write!(
                f,
                "line {line} starts with `{marker}`, and a highlight has no upgrade section; \
                 write the section in the fragment for the change, or, to show the token, indent \
                 the line or put the token in inline code"
            ),
        }
    }
}

impl std::error::Error for HighlightError {}

/// Read `source` as a highlight, held to every rule of the format.
///
/// Lines may end in `\n` or `\r\n`, and a leading byte order mark is skipped.
/// The title, each author, and the body are read without the whitespace around
/// them.
///
/// # Errors
///
/// Returns the first rule `source` breaks, reading from the top.
pub fn parse(source: &str) -> Result<Highlight, HighlightError> {
    let source = document::normalize(source);
    let lines: Vec<&str> = source.lines().collect();
    let has_text = |index: &usize| !lines[*index].trim().is_empty();

    let heading = (0..lines.len())
        .find(has_text)
        .ok_or(HighlightError::NoTitle)?;
    let title = title(lines[heading]).ok_or(HighlightError::NoTitle)?;

    let start = (heading + 1..lines.len())
        .find(has_text)
        .unwrap_or(lines.len());
    let (fields, end) = document::fields(&lines, start);

    let mut authors = vec![];
    let mut credits = vec![];
    for field in fields {
        if field.key.eq_ignore_ascii_case(AUTHORS) {
            if field.value.is_empty() {
                return Err(HighlightError::EmptyAuthor { line: field.line });
            }
            authors.push(field.value);
        } else if field.key.eq_ignore_ascii_case(CREDITS) {
            let credit = document::credit(&field).ok_or_else(|| HighlightError::InvalidCredit {
                line: field.line,
                value: field.value.clone(),
            })?;
            credits.push(credit);
        } else {
            return Err(HighlightError::UnknownField {
                line: field.line,
                key: field.key,
            });
        }
    }
    if authors.is_empty() {
        return Err(HighlightError::NoAuthors);
    }

    let offset = line_starts(&source)
        .get(end)
        .copied()
        .unwrap_or(source.len());
    let body = &source[offset..];
    let prose = Links::read(body).standalone(body, 0..body.len());
    let prose = prose.trim();
    if prose.is_empty() {
        return Err(HighlightError::EmptyBody);
    }

    let marked = body
        .lines()
        .enumerate()
        .find_map(|(index, line)| document::marker(line).map(|marker| (index, marker)));
    if let Some((index, marker)) = marked {
        return Err(HighlightError::Marker {
            line: end + index + 1,
            marker,
        });
    }

    Ok(Highlight {
        title: title.to_owned(),
        authors,
        credits: (!credits.is_empty()).then_some(credits),
        body: prose.to_owned(),
    })
}

/// The title of the `# Title` heading `line` holds, without the space around
/// it; `None` when `line` holds no such heading, or one with no title.
fn title(line: &str) -> Option<&str> {
    if !document::is_title(line) {
        return None;
    }

    let title = line.trim_start().strip_prefix('#')?.trim();
    (!title.is_empty()).then_some(title)
}

#[cfg(test)]
#[path = "highlight_tests.rs"]
mod tests;
