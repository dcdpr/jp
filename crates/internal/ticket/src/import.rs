//! Importing a ticket's content from an upstream tracker.
//!
//! The import is one-way and wholesale: the title, description, and comments
//! are replaced by whatever upstream says, and the metadata block is never
//! touched.
//! GitHub owns what was written on GitHub; the repository owns `Status`,
//! `Kind`, `Blocked by`, and `Implements`, so an imported ticket can be triaged
//! and moved across the board without the next import undoing it.
//!
//! Imported text is untrusted.
//! [`escape`] neutralises it before it reaches the working tree, so the site
//! renders it as data and never as page source.

use crate::{Comment, Kind};

/// What an upstream issue contributes to a ticket.
///
/// `kind`, `authors`, and `date` are only used when the import creates the
/// ticket; a later import of the same issue leaves the metadata block alone.
pub struct Import<'a> {
    /// The upstream issue number, recorded as `GitHub: #123`.
    pub number: u64,
    pub title: &'a str,
    pub description: &'a str,
    pub comments: Vec<Comment>,
    pub kind: Kind,
    pub authors: &'a str,
    pub date: &'a str,
}

impl Import<'_> {
    /// The `GitHub` metadata value that links a ticket to this issue.
    #[must_use]
    pub fn marker(&self) -> String {
        format!("#{}", self.number)
    }
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
