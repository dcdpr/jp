//! Parsing and validation for release highlights (`highlights/<slug>.md`).
//!
//! A highlight is an editorial section of the release notes for a feature worth
//! more than a single changelog bullet.
//! It starts with a `# Title` heading, followed by a metadata block with one or
//! more `- **Authors**:` fields (and optional `- **Credits**:` overrides), and
//! the section's prose body:
//!
//! ```markdown
//! # Unified editor and inline reply widget
//!
//! - **Authors**: @JeanMertz
//!
//! Every prompt in JP runs through a single editor service, and short replies
//! can be written inline without leaving the conversation view.
//! ```
//!
//! Highlights do not have a change [`Type`] or an upgrade section; the
//! corresponding fragment carries both.
//! Any line starting with `BREAKING CHANGE:` or `BREAKING-CHANGE:` is rejected
//! so upgrade instructions cannot be placed in a highlight by mistake.
//!
//! [`Type`]: crate::Type

use std::fmt;

use crate::{
    document::{self, CREDITS, Field, ListedCredit},
    links::{Links, line_starts},
};

const AUTHORS: &str = "Authors";

/// Parsed release highlight returned by [`parse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlight {
    /// Section heading in the release notes.
    pub title: String,

    /// Credited authors in declaration order (e.g. `@JeanMertz`).
    pub authors: Vec<String>,

    /// Explicit `- **Credits**:` overrides from the metadata block, if present.
    pub credits: Option<Vec<ListedCredit>>,

    /// Markdown prose body with reference links rewritten to inline
    /// `[text](url)` links.
    pub body: String,
}

/// Validation error returned by [`parse`].
/// Line numbers are 1-based from the start of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HighlightError {
    /// First non-empty line is not a non-empty `# Title` heading.
    NoTitle,

    /// Metadata block contains no `Authors` field.
    NoAuthors,

    /// An `Authors` field has an empty value.
    EmptyAuthor { line: usize },

    /// Metadata block contains a field other than `Authors` or `Credits`.
    UnknownField { line: usize, key: String },

    /// A `Credits` field value is neither a change request nor a commit hash.
    InvalidCredit { line: usize, value: String },

    /// No prose follows the title and metadata block.
    EmptyBody,

    /// A body line starts with an upgrade section marker.
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

/// Parse and validate a release highlight.
///
/// Normalizes `\r\n` line endings, strips a leading UTF-8 BOM, and trims
/// surrounding whitespace from the title, authors, and body.
///
/// # Errors
///
/// Returns the first [`HighlightError`] encountered in `source`.
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
    let (authors, credits) = authors_and_credits(fields)?;

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

fn authors_and_credits(
    fields: Vec<Field>,
) -> Result<(Vec<String>, Vec<ListedCredit>), HighlightError> {
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

    Ok((authors, credits))
}

/// Extract the trimmed heading text from a `# Title` line, returning `None` if
/// `line` is not a level-1 heading or has an empty title.
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
