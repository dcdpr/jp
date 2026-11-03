//! The document fragments and highlights are both written as: YAML frontmatter
//! between two `---` lines, then a Markdown body.
//!
//! [`parse`] splits a document and reads its frontmatter into the fields of its
//! format.
//! [`marker`] finds the lines both formats hold to the rules about upgrade
//! sections.

use std::fmt;

use serde::de::DeserializeOwned;

/// The tokens that open an upgrade section: the one Conventional Commits uses,
/// and its synonym.
const MARKERS: [&str; 2] = ["BREAKING CHANGE:", "BREAKING-CHANGE:"];

/// The line that opens and closes the frontmatter.
const DELIMITER: &str = "---";

/// Frontmatter that is missing, unclosed, or doesn't hold the fields of its
/// format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontmatterError {
    /// The first line isn't the `---` that opens the frontmatter.
    Missing,

    /// No `---` line closes the frontmatter.
    Unclosed,

    /// The frontmatter isn't YAML holding the fields of its format, and nothing
    /// else; the message is the YAML parser's, its lines counted from the top
    /// of the file.
    Invalid(String),
}

impl fmt::Display for FrontmatterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => f.write_str("no frontmatter: the first line has to be `---`"),
            Self::Unclosed => f.write_str("no `---` line closes the frontmatter"),
            Self::Invalid(message) => write!(f, "invalid frontmatter: {message}"),
        }
    }
}

impl std::error::Error for FrontmatterError {}

/// A document, its frontmatter read into the fields of its format.
pub(crate) struct Parsed<T> {
    pub(crate) frontmatter: T,

    /// Everything after the line closing the frontmatter, with `\n` line
    /// endings.
    pub(crate) body: String,

    /// The number of the body's first line in the file.
    pub(crate) body_line: usize,
}

/// Read `source` as a document whose frontmatter holds a `T`.
///
/// Lines may end in `\n` or `\r\n`, and a leading byte order mark is skipped.
pub(crate) fn parse<T: DeserializeOwned>(source: &str) -> Result<Parsed<T>, FrontmatterError> {
    let source = source
        .strip_prefix('\u{feff}')
        .unwrap_or(source)
        .replace("\r\n", "\n");

    let (yaml, body_start) = split(&source)?;
    let frontmatter = serde_saphyr::from_str(yaml).map_err(|error| {
        FrontmatterError::Invalid(
            error
                .without_snippet()
                .render_with_formatter(&serde_saphyr::UserMessageFormatter),
        )
    })?;

    Ok(Parsed {
        frontmatter,
        body: source[body_start..].to_owned(),
        body_line: line_number(&source, body_start),
    })
}

/// The token that opens an upgrade section `line` starts with, if it starts
/// with one.
pub(crate) fn marker(line: &str) -> Option<&'static str> {
    MARKERS.into_iter().find(|marker| line.starts_with(marker))
}

/// The frontmatter of `source`, and the offset its body starts at.
///
/// The frontmatter keeps its opening `---` line, so the YAML parser numbers
/// lines as the file does.
fn split(source: &str) -> Result<(&str, usize), FrontmatterError> {
    let mut lines = source.split_inclusive('\n');
    let opening = lines
        .next()
        .filter(|line| is_delimiter(line))
        .ok_or(FrontmatterError::Missing)?;

    let mut end = opening.len();
    for line in lines {
        if is_delimiter(line) {
            return Ok((&source[..end], end + line.len()));
        }
        end += line.len();
    }

    Err(FrontmatterError::Unclosed)
}

/// Whether `line` is the `---` that opens or closes the frontmatter, trailing
/// whitespace aside.
fn is_delimiter(line: &str) -> bool {
    line.trim_end() == DELIMITER
}

/// The number of the line `text` has reached at byte `offset`, counting from 1.
fn line_number(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count() + 1
}
