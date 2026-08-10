//! Importing a ticket's content from an upstream tracker.
//!
//! The import is one-way: the metadata block is never touched by it.
//! Upstream owns what was written upstream; the repository owns `Status`,
//! `Kind`, `Blocked by`, and `Implements`, so an imported ticket can be triaged
//! and moved across the board without the next import undoing it.
//!
//! [`Source`] names where a ticket came from and carries the value that links
//! it back, so re-importing finds the ticket it already wrote rather than
//! filing a second one.
//!
//! Imported text is untrusted.
//! [`escape`] neutralises it before it reaches the working tree, so the site
//! renders it as data and never as page source.

use std::fmt;

use crate::{Comment, Kind, Metadata};

/// Where an imported ticket came from.
///
/// The link lives in the ticket's metadata block, under the field the source
/// names, so the local id and the upstream identifier stay independent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A GitHub issue, recorded as `GitHub: #123`.
    GitHub { number: u64 },

    /// Anywhere else, recorded as `Source: scheme:id`.
    ///
    /// The scheme names the place and the id identifies the item there.
    /// The repository does not know what places exist: whatever wrote the
    /// ticket is the only thing that has to recognise its own scheme.
    External { scheme: String, id: String },
}

impl Source {
    /// Read an external source from its `scheme:id` spelling.
    ///
    /// The id may itself contain colons; only the first one divides the two.
    pub fn external(value: &str) -> Result<Self, SourceError> {
        let (scheme, id) = value
            .split_once(':')
            .ok_or_else(|| SourceError::NotAPair(value.to_owned()))?;

        let scheme = scheme.trim();
        let id = id.trim();
        if scheme.is_empty() || id.is_empty() {
            return Err(SourceError::NotAPair(value.to_owned()));
        }
        if !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        {
            return Err(SourceError::Scheme(scheme.to_owned()));
        }
        if let Some(char_) = id.chars().find(|c| !is_safe_in_a_marker(*c)) {
            return Err(SourceError::Id {
                id: id.to_owned(),
                char_,
            });
        }

        Ok(Self::External {
            scheme: scheme.to_owned(),
            id: id.to_owned(),
        })
    }

    /// The metadata field carrying the link.
    #[must_use]
    pub fn field(&self) -> &'static str {
        match self {
            Self::GitHub { .. } => "GitHub",
            Self::External { .. } => "Source",
        }
    }

    /// The value that field holds.
    #[must_use]
    pub fn marker(&self) -> String {
        match self {
            Self::GitHub { number } => format!("#{number}"),
            Self::External { scheme, id } => format!("{scheme}:{id}"),
        }
    }

    /// Whether a ticket carrying `metadata` was imported from here.
    #[must_use]
    pub fn links(&self, metadata: &Metadata) -> bool {
        let carried = match self {
            Self::GitHub { .. } => metadata.github.as_deref(),
            Self::External { .. } => metadata.source.as_deref(),
        };

        carried == Some(self.marker().as_str())
    }
}

/// Whether a character can appear in a marker written to a metadata line.
///
/// A marker is the one piece of imported text that reaches the working tree
/// unescaped, because it has to match byte for byte on the way back out.
/// Keeping it to plain single-line text is what makes that safe: [`escape`]
/// never has to run on it, so reading it back never has to undo anything.
fn is_safe_in_a_marker(char_: char) -> bool {
    !char_.is_control() && !matches!(char_, '<' | '{' | '}')
}

/// A `--source` value the format can't carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    /// Not a `scheme:id` pair, or one of the halves is empty.
    NotAPair(String),
    /// A scheme that isn't a bare word.
    Scheme(String),
    /// An id carrying a character a metadata line can't hold.
    Id { id: String, char_: char },
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAPair(value) => write!(
                f,
                "`{value}` is not a `scheme:id` pair naming where it came from."
            ),
            Self::Scheme(scheme) => write!(
                f,
                "`{scheme}` is not a source scheme; use letters, digits, `-`, and `_`."
            ),
            Self::Id { id, char_ } => {
                write!(f, "`{id}` cannot be a source id: it contains `{char_}`.")
            }
        }
    }
}

impl std::error::Error for SourceError {}

/// What an upstream item contributes to a ticket.
///
/// `kind`, `authors`, and `date` are only used when the import creates the
/// ticket; a later import of the same item leaves the metadata block alone.
pub struct Import<'a> {
    pub source: Source,
    pub title: &'a str,
    pub description: &'a str,
    pub comments: Vec<Comment>,
    pub kind: Kind,
    pub authors: &'a str,
    pub date: &'a str,
}

/// Neutralise untrusted markdown so the site renders it as content.
///
/// Ticket pages are compiled as Vue components, which gives imported text two
/// ways to break a build or reach the page as source: `{{ }}` interpolation,
/// and anything that parses as an HTML tag.
/// Both are escaped to entities, which markdown renders back as the literal
/// characters the author typed.
///
/// Escaping is unconditional, including inside fenced code blocks: telling
/// fences apart needs a parser, and an over-escaped entity in a code block is a
/// cosmetic problem where an unescaped tag is a correctness one.
#[must_use]
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(char_) = chars.next() {
        match char_ {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push_str("&#123;&#123;");
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push_str("&#125;&#125;");
            }
            // Only tag-like `<` is escaped, so arithmetic and prose survive.
            '<' if chars
                .peek()
                .is_some_and(|next| next.is_ascii_alphabetic() || matches!(next, '/' | '!')) =>
            {
                out.push_str("&lt;");
            }
            other => out.push(other),
        }
    }

    out
}

/// Apply [`escape`] to everything an import carries.
pub(crate) fn escaped(import: &Import<'_>) -> (String, String, Vec<Comment>) {
    let comments = import
        .comments
        .iter()
        .map(|comment| Comment {
            from: comment.from.clone(),
            date: comment.date.clone(),
            re: comment.re.clone(),
            body: escape(&comment.body),
        })
        .collect();

    (escape(import.title), escape(import.description), comments)
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
