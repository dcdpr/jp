//! One highlight: an editorial section of the release notes, for a feature
//! worth more than its entry.
//!
//! A highlight is a Markdown file in a `highlights/` directory, whose YAML
//! frontmatter names the section's title and the people it credits, and whose
//! body is the section's prose:
//!
//! ```markdown
//! ---
//! title: "Unified editor and inline reply widget"
//! authors: ["@JeanMertz"]
//! ---
//! Every prompt in JP now runs through a single editor service, and short
//! replies can be written inline without leaving the conversation view.
//! ```
//!
//! A highlight has no type and no upgrade section: the fragment for the change
//! carries both.
//! A line that starts with an upgrade section's token is an error, so a section
//! meant for the upgrade guide can't end up in the release notes' prose
//! instead.

use std::{fmt, num::NonZeroU64};

use serde::Deserialize;

use crate::{
    document::{self, FrontmatterError, Parsed},
    links::Links,
};

/// A highlight, as [`parse`] reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlight {
    /// The heading of the highlight's section in the release notes.
    pub title: String,

    /// The people credited for the feature, as written, such as `@JeanMertz`.
    pub authors: Vec<String>,

    /// The change requests to credit in place of the ones history names, when
    /// the frontmatter lists them.
    pub change_requests: Option<Vec<NonZeroU64>>,

    /// The section's prose, as Markdown.
    ///
    /// Its links are inline, `[text](url)`, whichever form the file wrote them
    /// in, so the prose stands alone beside other highlights.
    pub body: String,
}

/// A document that doesn't hold a well-formed highlight.
///
/// Lines count from 1 at the top of the file, frontmatter included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HighlightError {
    /// The frontmatter is missing, unclosed, or doesn't hold a highlight's
    /// fields and nothing else.
    Frontmatter(FrontmatterError),

    /// `title` holds nothing but whitespace.
    EmptyTitle,

    /// `authors` lists nobody.
    NoAuthors,

    /// An entry of `authors` holds nothing but whitespace.
    EmptyAuthor,

    /// Nothing but whitespace follows the frontmatter.
    EmptyBody,

    /// A line starts with an upgrade section's token.
    Marker { line: usize, marker: &'static str },
}

impl fmt::Display for HighlightError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Frontmatter(error) => write!(f, "{error}"),
            Self::EmptyTitle => f.write_str(
                "no title: name the feature in `title`, which heads its section of the release \
                 notes",
            ),
            Self::NoAuthors => {
                f.write_str("no authors: list the people to credit for the feature in `authors`")
            }
            Self::EmptyAuthor => {
                f.write_str("an empty author: each entry of `authors` names someone to credit")
            }
            Self::EmptyBody => {
                f.write_str("no prose: write below the frontmatter what the feature does for users")
            }
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

impl From<FrontmatterError> for HighlightError {
    fn from(error: FrontmatterError) -> Self {
        Self::Frontmatter(error)
    }
}

/// The fields a highlight's frontmatter holds; any other is an error rather
/// than ignored, so a misspelled field doesn't vanish, and neither does a
/// fragment's `type`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Frontmatter {
    title: String,
    authors: Vec<String>,
    change_requests: Option<Vec<NonZeroU64>>,
}

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
    let Parsed {
        frontmatter,
        body,
        body_line,
    } = document::parse::<Frontmatter>(source)?;

    let title = frontmatter.title.trim();
    if title.is_empty() {
        return Err(HighlightError::EmptyTitle);
    }

    if frontmatter.authors.is_empty() {
        return Err(HighlightError::NoAuthors);
    }
    let authors: Vec<String> = frontmatter
        .authors
        .iter()
        .map(|author| author.trim().to_owned())
        .collect();
    if authors.iter().any(String::is_empty) {
        return Err(HighlightError::EmptyAuthor);
    }

    let prose = Links::read(&body).standalone(&body, 0..body.len());
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
            line: body_line + index,
            marker,
        });
    }

    Ok(Highlight {
        title: title.to_owned(),
        authors,
        change_requests: frontmatter.change_requests,
        body: prose.to_owned(),
    })
}

#[cfg(test)]
#[path = "highlight_tests.rs"]
mod tests;
