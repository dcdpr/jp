//! A document's headings, and the anchors the site gives them.
//!
//! The site renders a document with VitePress, which gives every heading an
//! `id` for `#fragment` links: the heading's text through [`slugify`], with
//! `-1`, `-2`, and so on appended when an earlier heading in the document
//! already took the slug.
//! [`headings`] reads a document the same way, so a fragment matching one of
//! its slugs is one the site renders.
//!
//! What the site leaves out of a heading's text is left out here too: inline
//! HTML, and the markup of emphasis, links, and code spans.
//! Headings the site anchors count wherever they sit, in a block quote or a
//! list item, and in either the `#` or the underlined form; a line inside a
//! code block or the frontmatter is no heading.
//!
//! `fixtures/heading-slugs.json` lists headings and the slugs the site gives
//! them, as JSON a test on the site's side can read as well.

use std::collections::HashSet;

use comrak::{
    Arena, Options,
    nodes::{AstNode, NodeValue},
    options::Extension,
    parse_document,
};
use unicode_normalization::UnicodeNormalization as _;

/// The whitespace in JavaScript's `\s` that isn't a control character, outside
/// the run of spaces from U+2000 to U+200A.
const WHITESPACE: [char; 9] = [
    ' ', '\u{a0}', '\u{1680}', '\u{2028}', '\u{2029}', '\u{202f}', '\u{205f}', '\u{3000}',
    '\u{feff}',
];

/// The curly quotes, the only punctuation outside ASCII a slug replaces.
const CURLY_QUOTES: [char; 4] = ['\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}'];

/// One heading of a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// 1 for a `#` heading, up to 6 for `######`.
    pub level: u8,

    /// The text the anchor is computed from: the heading's content without its
    /// markup.
    pub text: String,

    /// The anchor, unique within the document.
    pub slug: String,

    /// The 0-based index of the line the heading starts on.
    pub line: usize,
}

/// Every heading in `source`, in document order, with its anchor.
#[must_use]
pub fn headings(source: &str) -> Vec<Heading> {
    let arena = Arena::new();
    let root = parse_document(&arena, source, &options());

    let mut slugs = Slugs::default();
    root.descendants()
        .filter_map(|node| {
            let data = node.data();
            let NodeValue::Heading(heading) = &data.value else {
                return None;
            };
            let text = text_of(node);

            Some(Heading {
                level: heading.level,
                slug: slugs.claim(&slugify(&text)),
                text,
                // Positions count lines from 1, frontmatter included.
                line: data.sourcepos.start.line.saturating_sub(1),
            })
        })
        .collect()
}

/// The slug the site computes from a heading's text.
///
/// A port of VitePress's default, `slugify` from `@mdit-vue/shared`: the text
/// is decomposed (NFKD) and stripped of combining accents and control
/// characters, each run of whitespace and punctuation becomes one `-`, a `-` at
/// either end is dropped, a leading digit gets a `_` in front, and the result
/// is lowercased.
/// Letters outside ASCII are kept.
#[must_use]
pub fn slugify(text: &str) -> String {
    let mut slug = String::with_capacity(text.len());
    let mut separated = false;

    for char_ in text.nfkd() {
        if is_combining_accent(char_) || is_control(char_) {
            continue;
        }
        if is_separator(char_) {
            separated = true;
            continue;
        }

        // A run of separators becomes one dash, and none at the start; none at
        // the end either, since nothing follows the last run to write it.
        if separated && !slug.is_empty() {
            slug.push('-');
        }
        separated = false;
        slug.push(char_);
    }

    if slug.starts_with(|char_: char| char_.is_ascii_digit()) {
        slug.insert(0, '_');
    }

    slug.to_lowercase()
}

/// The slugs a document's headings took so far.
#[derive(Default)]
struct Slugs(HashSet<String>);

impl Slugs {
    /// `slug` when no earlier heading took it, else the first of `slug-1`,
    /// `slug-2`, and so on that none did.
    ///
    /// The suffixed form is checked against every slug taken, so a heading
    /// whose own slug is `a-1` can push a later duplicate to `a-1-1`.
    fn claim(&mut self, slug: &str) -> String {
        let mut unique = slug.to_owned();
        let mut suffix = 1;
        while self.0.contains(&unique) {
            unique = format!("{slug}-{suffix}");
            suffix += 1;
        }

        self.0.insert(unique.clone());
        unique
    }
}

/// Parse options that read a document the way the site does: tables and
/// strikethrough on, and a leading `---` block read as frontmatter rather than
/// as a rule and a heading.
fn options() -> Options<'static> {
    Options {
        extension: Extension {
            strikethrough: true,
            table: true,
            front_matter_delimiter: Some("---".to_owned()),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The text of a heading node, the way the site reads it for the anchor: text
/// and code spans, and nothing of inline HTML or of the markup around them.
fn text_of<'a>(heading: &'a AstNode<'a>) -> String {
    let mut text = String::new();
    for node in heading.descendants() {
        match &node.data().value {
            NodeValue::Text(literal) => text.push_str(literal),
            NodeValue::Code(code) => text.push_str(&code.literal),
            _ => {}
        }
    }

    text
}

/// The combining diacritical marks NFKD splits off an accented letter.
fn is_combining_accent(char_: char) -> bool {
    ('\u{0300}'..='\u{036f}').contains(&char_)
}

/// The C0 control characters, tabs and line breaks among them.
fn is_control(char_: char) -> bool {
    char_ <= '\u{001f}'
}

/// Whitespace and the punctuation a slug replaces with `-`: all of ASCII's, and
/// the curly quotes.
fn is_separator(char_: char) -> bool {
    char_.is_ascii_punctuation()
        || WHITESPACE.contains(&char_)
        || ('\u{2000}'..='\u{200a}').contains(&char_)
        || CURLY_QUOTES.contains(&char_)
}

#[cfg(test)]
#[path = "heading_tests.rs"]
mod tests;
