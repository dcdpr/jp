//! A rendered file, split into the blocks it is drawn as, an element each.
//!
//! `md-html` renders Markdown as a sequence of top-level elements (headings,
//! paragraphs, lists, tables, code blocks) and any other file as one `<pre>`
//! holding a `<span>` per line.
//! Each top-level element is a block, and so is each line of a plain-text file.
//! A highlight that changes has the blocks it left or reached drawn again
//! (`crate::highlight`), and no more of the file than those.
//! Splitting keeps every `data-s`/`data-e` span intact.

/// Elements without an end tag.
const VOID: [&str; 6] = ["br", "hr", "img", "input", "col", "wbr"];

/// What opens an HTML comment.
const COMMENT_OPEN: &[u8] = b"<!--";

/// What closes an HTML comment.
const COMMENT_CLOSE: &[u8] = b"-->";

/// One block of a rendered file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// The block's HTML, as `md-html` wrote it.
    pub html: String,

    /// Whether the block is one line of a file rendered as plain text, which is
    /// shown preformatted.
    pub line: bool,
}

/// The blocks of `html`, the output of `md-html`, in document order.
#[must_use]
pub fn blocks(html: &str) -> Vec<Block> {
    let mut blocks = vec![];
    for element in top_level(html) {
        match lines(element) {
            Some(lines) => blocks.extend(lines.map(|line| block(line, true))),
            None => blocks.push(block(element, false)),
        }
    }

    blocks
}

fn block(html: &str, line: bool) -> Block {
    Block {
        html: html.to_owned(),
        line,
    }
}

/// Each top-level element of `html`, and each run of text between them that
/// holds more than whitespace.
fn top_level(html: &str) -> Vec<&str> {
    let mut elements = vec![];
    let mut depth = 0_usize;
    let mut start = 0;

    for token in tokens(html) {
        let Kind::Tag(tag) = token.kind else {
            let text = html[token.start..token.end].trim();
            if depth == 0 && !text.is_empty() {
                elements.push(text);
            }
            continue;
        };

        if depth == 0 {
            start = token.start;
        }
        depth = match tag {
            Tag::Open => depth + 1,
            Tag::Close => depth.saturating_sub(1),
            Tag::Void => depth,
        };
        if depth == 0 {
            elements.push(&html[start..token.end]);
        }
    }

    elements
}

/// A tag, or a run of text between tags, by its byte range in the HTML.
struct Token {
    start: usize,
    end: usize,
    kind: Kind,
}

enum Kind {
    Text,
    Tag(Tag),
}

/// What a tag does to the nesting depth.
enum Tag {
    Open,
    Close,
    Void,
}

/// The tags and runs of text `html` is made of, in order.
fn tokens(html: &str) -> Vec<Token> {
    let bytes = html.as_bytes();
    let mut tokens = vec![];
    let mut index = 0;

    while index < bytes.len() {
        let (end, kind) = if bytes[index] == b'<' {
            let end = tag_end(bytes, index);
            (end, Kind::Tag(tag_kind(&html[index..end])))
        } else {
            let next = bytes[index..].iter().position(|byte| *byte == b'<');
            (
                next.map_or(bytes.len(), |offset| index + offset),
                Kind::Text,
            )
        };
        tokens.push(Token {
            start: index,
            end,
            kind,
        });
        index = end;
    }

    tokens
}

fn tag_kind(tag: &str) -> Tag {
    if tag.starts_with("</") {
        return Tag::Close;
    }
    if tag.starts_with("<!") || tag.ends_with("/>") {
        return Tag::Void;
    }

    let name: String = tag[1..]
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect();
    if VOID.contains(&name.to_ascii_lowercase().as_str()) {
        Tag::Void
    } else {
        Tag::Open
    }
}

/// The index just past the tag starting at `start`, skipping a `>` inside a
/// quoted attribute value.
fn tag_end(bytes: &[u8], start: usize) -> usize {
    if bytes[start..].starts_with(COMMENT_OPEN) {
        return find(bytes, start + COMMENT_OPEN.len(), COMMENT_CLOSE)
            .map_or(bytes.len(), |end| end + COMMENT_CLOSE.len());
    }

    let mut quote = None;
    for (index, &byte) in bytes.iter().enumerate().skip(start + 1) {
        match (quote, byte) {
            (None, b'"' | b'\'') => quote = Some(byte),
            (Some(open), _) if byte == open => quote = None,
            (None, b'>') => return index + 1,
            _ => {}
        }
    }

    bytes.len()
}

fn find(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    bytes
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|offset| from + offset)
}

/// The lines of `element` when it is a file rendered as plain text: a `<pre>`
/// holding a `<span>` per line, separated by line breaks.
fn lines(element: &str) -> Option<impl Iterator<Item = &str>> {
    let inner = element
        .strip_prefix("<pre><span ")
        .map(|_| &element["<pre>".len()..])?
        .strip_suffix("</pre>")?;

    Some(inner.split('\n').filter(|line| !line.is_empty()))
}

#[cfg(test)]
#[path = "blocks_tests.rs"]
mod tests;
