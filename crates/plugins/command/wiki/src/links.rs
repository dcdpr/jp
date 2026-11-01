//! What a document links to, as markdown reads it.
//!
//! [`links`] finds every link and image, reference-style ones resolved against
//! the document's own definitions, and notes which images stand alone in their
//! paragraph: those are embeds.
//! [`mentions`] finds the documents its prose names by id, such as `W-0a1b2c3`,
//! `RFD 042`, or `T-02wt0kx`, each optionally followed by a `#section`.
//! The rest are helpers for reading a destination: its fragment, the path it
//! resolves to, and the line range a fragment names.

use std::{ops::Range, sync::LazyLock};

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use comrak::{
    Arena, Options,
    nodes::{AstNode, NodeValue, Sourcepos},
    options::Extension,
    parse_document,
};
use regex::Regex;

/// A document named by id: `W-0a1b2c3`, `T-02wt0kx`, or `RFD 042`, with an
/// optional `#section`.
///
/// Groups: the prefix of a wiki page or ticket id, its body, the RFD's id, and
/// the fragment.
static MENTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:([WT])-([0-9a-z]{7})\b|RFD\s+(\d{3}|D\d{2})\b)(?:#([\w-]+))?")
        .expect("valid regex")
});

/// One link or image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Link {
    /// The destination, a reference-style one resolved.
    pub url: String,

    pub title: String,

    pub image: bool,

    /// The label, when it is nothing but a code span: `by_rfd` for
    /// ``[`by_rfd`]``.
    pub code_label: Option<String>,

    /// The 0-based line the link starts on.
    pub line: usize,

    /// Where the link is in the source, as a byte range.
    pub span: Range<usize>,

    /// Where its text is in the source; `None` for a link without text.
    pub text: Option<Range<usize>>,

    /// Whether the destination is written in the link, `[a](b)`, rather than in
    /// a definition elsewhere, `[a][b]`.
    pub inline: bool,

    /// For an image alone in its paragraph, an embed, the 0-based lines of that
    /// paragraph.
    pub standalone: Option<Range<usize>>,
}

/// What a mention names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Named {
    Wiki,
    Rfd,
    Ticket,
}

/// A document named by id in prose or a code span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mention {
    pub named: Named,

    /// The id as written canonically: `W-0a1b2c3`, `042`, `T-02wt0kx`.
    pub id: String,

    pub fragment: Option<String>,

    /// The 0-based line of the block the mention is in.
    pub line: usize,
}

/// Parse options that read a document the way the site does: tables and
/// strikethrough on, and a leading `---` block read as frontmatter.
pub(crate) fn parse_options() -> Options<'static> {
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

/// Every link and image in `source`, in document order.
#[must_use]
pub(crate) fn links(source: &str) -> Vec<Link> {
    let arena = Arena::new();
    let root = parse_document(&arena, source, &parse_options());
    let starts = line_starts(source);

    let mut found = vec![];
    for node in root.descendants() {
        let data = node.data();
        let (url, title, image) = match &data.value {
            NodeValue::Link(link) => (link.url.clone(), link.title.clone(), false),
            NodeValue::Image(link) => (link.url.clone(), link.title.clone(), true),
            _ => continue,
        };
        let Some(span) = byte_range(&starts, source.len(), &data.sourcepos) else {
            continue;
        };

        let written = &source[span.clone()];
        found.push(Link {
            url,
            title,
            image,
            code_label: code_label(node),
            line: data.sourcepos.start.line.saturating_sub(1),
            text: text_range(node, &starts, source.len()),
            inline: written.ends_with(')') || written.starts_with('<'),
            standalone: image.then(|| standalone(node)).flatten(),
            span,
        });
    }

    found
}

/// Every document `source` names by id, outside code blocks, in order.
///
/// A mention wrapped across a line break, `RFD` at the end of one line and
/// `042` at the start of the next, still counts.
#[must_use]
pub(crate) fn mentions(source: &str) -> Vec<Mention> {
    let arena = Arena::new();
    let root = parse_document(&arena, source, &parse_options());

    let mut found = vec![];
    for node in root.descendants() {
        let data = node.data();
        if !matches!(
            data.value,
            NodeValue::Paragraph | NodeValue::Heading(_) | NodeValue::TableCell
        ) {
            continue;
        }

        let line = data.sourcepos.start.line.saturating_sub(1);
        found.extend(mentions_in(&inline_text(node), line));
    }

    found
}

/// The mentions in `text`, all on `line`.
fn mentions_in(text: &str, line: usize) -> Vec<Mention> {
    MENTION
        .captures_iter(text)
        .map(|captures| {
            let (named, id) = match (captures.get(1), captures.get(2), captures.get(3)) {
                (Some(prefix), Some(body), _) if prefix.as_str() == "W" => {
                    (Named::Wiki, format!("W-{}", body.as_str()))
                }
                (Some(_), Some(body), _) => (Named::Ticket, format!("T-{}", body.as_str())),
                (_, _, rfd) => (
                    Named::Rfd,
                    rfd.map(|id| id.as_str().to_owned()).unwrap_or_default(),
                ),
            };

            Mention {
                named,
                id,
                fragment: captures.get(4).map(|fragment| fragment.as_str().to_owned()),
                line,
            }
        })
        .collect()
}

/// The text of a block's inline content: its text and code spans, with line
/// breaks read as spaces.
fn inline_text<'a>(block: &'a AstNode<'a>) -> String {
    let mut text = String::new();
    for node in block.descendants() {
        match &node.data().value {
            NodeValue::Text(literal) => text.push_str(literal),
            NodeValue::Code(code) => text.push_str(&code.literal),
            NodeValue::SoftBreak | NodeValue::LineBreak => text.push(' '),
            _ => {}
        }
    }

    text
}

/// The label of a link whose text is nothing but one code span.
fn code_label<'a>(link: &'a AstNode<'a>) -> Option<String> {
    let mut children = link.children().filter(
        |child| !matches!(&child.data().value, NodeValue::Text(text) if text.trim().is_empty()),
    );
    let only = children.next()?;
    if children.next().is_some() {
        return None;
    }

    match &only.data().value {
        NodeValue::Code(code) => Some(code.literal.clone()),
        _ => None,
    }
}

/// Where a link's text is in the source: from its first child to its last.
fn text_range<'a>(link: &'a AstNode<'a>, starts: &[usize], size: usize) -> Option<Range<usize>> {
    let first = byte_range(starts, size, &link.first_child()?.data().sourcepos)?;
    let last = byte_range(starts, size, &link.last_child()?.data().sourcepos)?;

    (first.start <= last.end).then_some(first.start..last.end)
}

/// The lines of the paragraph `image` stands alone in, or `None` when it shares
/// the paragraph with anything but whitespace.
fn standalone<'a>(image: &'a AstNode<'a>) -> Option<Range<usize>> {
    let paragraph = image.parent()?;
    if !matches!(paragraph.data().value, NodeValue::Paragraph) {
        return None;
    }

    let alone = paragraph.children().all(|child| {
        std::ptr::eq(child, image)
            || matches!(&child.data().value, NodeValue::Text(text) if text.trim().is_empty())
            || matches!(
                child.data().value,
                NodeValue::SoftBreak | NodeValue::LineBreak
            )
    });
    if !alone {
        return None;
    }

    let position = paragraph.data().sourcepos;
    Some(position.start.line.saturating_sub(1)..position.end.line)
}

/// The byte offset each line of `source` starts at.
#[must_use]
pub(crate) fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(
        source
            .bytes()
            .enumerate()
            .filter(|(_, byte)| *byte == b'\n')
            .map(|(index, _)| index + 1),
    );

    starts
}

/// A comrak position (1-based line, 1-based byte column, end inclusive) as a
/// half-open byte range, or `None` when it falls outside the source.
fn byte_range(starts: &[usize], size: usize, position: &Sourcepos) -> Option<Range<usize>> {
    let start = starts
        .get(position.start.line.checked_sub(1)?)?
        .checked_add(position.start.column.saturating_sub(1))?;
    let end = starts
        .get(position.end.line.checked_sub(1)?)?
        .checked_add(position.end.column)?;

    (start <= end && end <= size).then_some(start..end)
}

/// Whether `url` names a scheme, as `https:` and `mailto:` do.
#[must_use]
pub(crate) fn has_scheme(url: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };

    scheme.len() > 1
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// A destination's path and its fragment, without the `#`.
#[must_use]
pub(crate) fn split_fragment(url: &str) -> (&str, Option<&str>) {
    match url.split_once('#') {
        Some((path, fragment)) => (path, Some(fragment)),
        None => (url, None),
    }
}

/// `path` with `%XX` escapes decoded, as a browser reads a link.
#[must_use]
pub(crate) fn percent_decode(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = bytes
            .get(index + 1..index + 3)
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (bytes[index], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                index += 3;
            }
            (byte, _) => {
                out.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

/// `path` with its `.` and `..` components worked out, as text: no symlink is
/// followed.
#[must_use]
pub(crate) fn normalize(path: &Utf8Path) -> Utf8PathBuf {
    let mut out = Utf8PathBuf::new();
    for component in path.components() {
        match component {
            Utf8Component::CurDir => {}
            Utf8Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_str()),
        }
    }

    out
}

/// Where the relative destination `path` leads from a document in `dir`.
#[must_use]
pub(crate) fn resolve(dir: &Utf8Path, path: &str) -> Utf8PathBuf {
    normalize(&dir.join(percent_decode(path)))
}

/// A line range a fragment names: `L12`, `L12-L24`, `L12-24`, or `12-24`, as
/// 1-based lines, start first.
#[must_use]
pub(crate) fn line_range(fragment: &str) -> Option<(usize, usize)> {
    let number = |text: &str| -> Option<usize> {
        (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse().ok())
            .flatten()
    };

    if let Some(rest) = fragment.strip_prefix('L') {
        return match rest.split_once('-') {
            None => number(rest).map(|line| (line, line)),
            Some((start, end)) => Some((
                number(start)?,
                number(end.strip_prefix('L').unwrap_or(end))?,
            )),
        };
    }

    let (start, end) = fragment.split_once('-')?;
    Some((number(start)?, number(end)?))
}

/// Whether `source` holds `word`, an id such as `W-0a1b2c3`, with no letter,
/// digit, or `_` on either side.
#[must_use]
pub(crate) fn names(source: &str, word: &str) -> bool {
    let continues = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');

    source.match_indices(word).any(|(at, _)| {
        !continues(source[..at].chars().next_back())
            && !continues(source[at + word.len()..].chars().next())
    })
}

/// The symbol a code-span label guards: its trailing identifier, `by_rfd` for
/// `Links::by_rfd`.
#[must_use]
pub(crate) fn symbol(label: &str) -> Option<&str> {
    label
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .rfind(|part| !part.is_empty())
}

#[cfg(test)]
#[path = "links_tests.rs"]
mod tests;
