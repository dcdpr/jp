//! Where a link in a rendered file leads.
//!
//! A file's links are rendered as its Markdown writes them, relative to the
//! file: `139-other.md`, `../notes.md#usage`, `#commenting`.
//! The page's own address names a review and not the file, so the browser would
//! resolve them to addresses the server has nothing at.
//! [`resolve`] reads a link as the file means it, for the page to follow it
//! itself.

use crate::route::unescape;

/// Where a link leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A heading of the file the link is in, by its anchor: `commenting`.
    Heading(String),

    /// A file of the workspace, by its path from the workspace root, and the
    /// anchor of the heading in it the link names.
    File {
        path: String,
        heading: Option<String>,
    },

    /// An address of its own: `https://example.com`, `mailto:jane@example.com`.
    Elsewhere,

    /// Nothing the workspace holds: a path that climbs above the workspace
    /// root, one from the root of a site (`/rfd/`), or no address at all.
    Nowhere,
}

/// Where the link `href` leads, written in the file at `from`, a path from the
/// workspace root.
///
/// A query on a link to a file is dropped: a file has none.
#[must_use]
pub fn resolve(href: &str, from: &str) -> Target {
    let href = href.trim();
    if href.is_empty() || (href.starts_with('/') && !href.starts_with("//")) {
        return Target::Nowhere;
    }
    if let Some(heading) = href.strip_prefix('#') {
        return Target::Heading(unescape(heading, false));
    }
    if href.starts_with("//") || has_scheme(href) {
        return Target::Elsewhere;
    }

    let (path, heading) = match href.split_once('#') {
        Some((path, heading)) => (path, Some(heading)),
        None => (href, None),
    };
    let path = path.split_once('?').map_or(path, |(path, _)| path);
    let Some(path) = beside(from, &unescape(path, false)) else {
        return Target::Nowhere;
    };

    Target::File {
        path,
        heading: heading
            .filter(|heading| !heading.is_empty())
            .map(|heading| unescape(heading, false)),
    }
}

/// The path from the workspace root that `path` names, read from the directory
/// the file at `from` is in, with `.` and `..` folded away; `None` when it
/// climbs above the root, or names the root itself.
fn beside(from: &str, path: &str) -> Option<String> {
    let mut parts: Vec<&str> = from.split('/').collect();
    // The file's own name, which leaves its directory.
    parts.pop();

    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }

    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Whether `href` opens with a scheme, such as `https:` or `mailto:`.
fn has_scheme(href: &str) -> bool {
    let Some((scheme, _)) = href.split_once(':') else {
        return false;
    };
    let mut characters = scheme.chars();

    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '.' | '-')
        })
}

#[cfg(test)]
#[path = "link_tests.rs"]
mod tests;
