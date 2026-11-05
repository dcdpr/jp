//! Reference-style links, rewritten so any part of a document stands alone.
//!
//! A reference-style link, `[text][label]` or `[label]`, reads its URL from a
//! definition, `[label]: url`, which can sit anywhere in the document.
//! A formatter that gathers the definitions at the end of the file puts them
//! below a fragment's upgrade section, so the entry, read apart from them,
//! links nowhere.
//! And two documents placed one after the other can each define a label
//! differently, in which case the first definition wins for both.
//!
//! [`Links::read`] finds what to change, and [`Links::standalone`] writes any
//! part of the document with its reference-style links and images inline,
//! `[text](url)`, and its definitions gone.

use std::{iter, ops::Range};

use comrak::{
    Arena,
    nodes::{AstNode, NodeValue, Sourcepos},
    parse_document,
};

use crate::document;

/// The changes that make a document's links stand alone.
pub(crate) struct Links {
    /// Byte ranges of the document, each with the text that replaces it, in
    /// order and none overlapping.
    edits: Vec<(Range<usize>, String)>,
}

impl Links {
    /// Find the reference-style links and images in `markdown`, and the
    /// definitions they read.
    ///
    /// The definitions are dropped only when every link could be rewritten, so
    /// a link left as written keeps the definition it reads.
    pub(crate) fn read(markdown: &str) -> Self {
        let arena = Arena::new();
        let root = parse_document(&arena, markdown, &document::markdown_options());
        let starts = line_starts(markdown);

        let mut edits = vec![];
        let mut every_link = true;
        for node in root.descendants() {
            let data = node.data();
            let (url, title, image) = match &data.value {
                NodeValue::Link(link) => (&link.url, &link.title, false),
                NodeValue::Image(link) => (&link.url, &link.title, true),
                _ => continue,
            };

            match form(markdown, &starts, &data.sourcepos, image) {
                Some(Form::Inline) => {}
                Some(Form::Reference(label)) => edits.push((label, inline(url, title))),
                None => every_link = false,
            }
        }

        if every_link {
            let definitions = definitions(root, markdown, &starts);
            edits.extend(definitions.into_iter().map(|line| (line, String::new())));
        }
        edits.sort_by_key(|(range, _)| range.start);

        Self { edits }
    }

    /// `markdown[range]`, with its links inline and its definitions gone.
    ///
    /// `markdown` is the document [`Links::read`] read, and `range` starts and
    /// ends at a line: no link or definition spans two such parts.
    pub(crate) fn standalone(&self, markdown: &str, range: Range<usize>) -> String {
        let mut out = String::with_capacity(range.len());
        let mut cursor = range.start;
        for (edit, text) in &self.edits {
            if edit.start < cursor || edit.end > range.end {
                continue;
            }
            out.push_str(&markdown[cursor..edit.start]);
            out.push_str(text);
            cursor = edit.end;
        }
        out.push_str(&markdown[cursor..range.end]);

        out
    }
}

/// The byte offset each line of `text` starts at, the first line's included.
pub(crate) fn line_starts(text: &str) -> Vec<usize> {
    iter::once(0)
        .chain(text.match_indices('\n').map(|(index, _)| index + 1))
        .collect()
}

/// How a link or image is written.
enum Form {
    /// Inline, `[text](url)`, or an autolink, `<url>`: nothing to change.
    Inline,

    /// In reference form, with the byte range after its text that inline form
    /// replaces: the label, `[label]` or `[]`, or nothing at all when the text
    /// is the label.
    Reference(Range<usize>),
}

/// How the link or image at `sourcepos` in `markdown` is written.
///
/// `None` when its source can't be read as a link, which leaves it as written.
fn form(markdown: &str, starts: &[usize], sourcepos: &Sourcepos, image: bool) -> Option<Form> {
    let range = byte_range(markdown, starts, sourcepos)?;
    let source = &markdown[range.clone()];
    if source.starts_with('<') {
        return Some(Form::Inline);
    }

    let text = if image {
        source.strip_prefix('!')?
    } else {
        source
    };
    let close = closing_bracket(text)?;
    let label = &text[close + 1..];
    if label.starts_with('(') {
        return Some(Form::Inline);
    }

    // A label is one bracketed run reaching the end, or nothing: anything else
    // means the text's closing bracket was misread.
    if !label.is_empty() && closing_bracket(label) != Some(label.len() - 1) {
        return None;
    }

    Some(Form::Reference(range.end - label.len()..range.end))
}

/// The byte range `sourcepos` spans in `markdown`.
///
/// comrak counts lines and columns from 1, and includes the end column.
fn byte_range(markdown: &str, starts: &[usize], sourcepos: &Sourcepos) -> Option<Range<usize>> {
    let start = starts.get(sourcepos.start.line.checked_sub(1)?)?
        + sourcepos.start.column.checked_sub(1)?;
    let end = starts.get(sourcepos.end.line.checked_sub(1)?)? + sourcepos.end.column;

    let readable = start < end
        && end <= markdown.len()
        && markdown.is_char_boundary(start)
        && markdown.is_char_boundary(end);

    readable.then_some(start..end)
}

/// The byte index of the `]` that closes the `[` `text` opens with.
///
/// Brackets nest, a backslash escapes the byte after it, and brackets inside a
/// code span don't count, as CommonMark reads a link's text.
fn closing_bracket(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'[') {
        return None;
    }

    let mut depth = 0_usize;
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'\\' => index += 1,
            b'`' => {
                let run = backticks(bytes, index);
                index = closing_run(bytes, index + run, run).unwrap_or(index + run - 1);
            }
            b'[' => depth += 1,
            b']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }

    None
}

/// How many backticks run from `index` in `bytes`.
fn backticks(bytes: &[u8], index: usize) -> usize {
    bytes
        .iter()
        .skip(index)
        .take_while(|&&byte| byte == b'`')
        .count()
}

/// The index of the last backtick of the first run of exactly `count` at or
/// after `from`: where a code span opened by as many backticks closes.
fn closing_run(bytes: &[u8], from: usize, count: usize) -> Option<usize> {
    let mut index = from;
    while let Some(&byte) = bytes.get(index) {
        if byte != b'`' {
            index += 1;
            continue;
        }

        let run = backticks(bytes, index);
        if run == count {
            return Some(index + run - 1);
        }
        index += run;
    }

    None
}

/// The inline form of a link to `url` titled `title`: `(url "title")`.
fn inline(url: &str, title: &str) -> String {
    let destination = if bare(url) {
        url.to_owned()
    } else {
        format!("<{}>", escaped(url, &['\\', '<', '>']))
    };
    if title.is_empty() {
        return format!("({destination})");
    }

    // On one line, so no text the rewrite moves can start a line of its own.
    let title = escaped(&title.replace('\n', " "), &['\\', '"']);
    format!("({destination} \"{title}\")")
}

/// Whether `url` can be written as it is, outside angle brackets: it holds no
/// whitespace, `<`, `>`, or `\`, and its parentheses balance.
fn bare(url: &str) -> bool {
    let mut depth = 0_usize;
    for c in url.chars() {
        match c {
            '(' => depth += 1,
            ')' => match depth.checked_sub(1) {
                Some(outer) => depth = outer,
                None => return false,
            },
            '<' | '>' | '\\' => return false,
            c if c.is_whitespace() || c.is_control() => return false,
            _ => {}
        }
    }

    !url.is_empty() && depth == 0
}

/// `text` with a backslash before each of the `special` characters.
fn escaped(text: &str, special: &[char]) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if special.contains(&c) {
            out.push('\\');
        }
        out.push(c);
    }

    out
}

/// The lines holding the document's top-level definitions, each as the byte
/// range of the line and its line break.
///
/// A definition leaves no node behind, so a line no top-level block covers
/// holds one, or nothing.
/// One inside a list or a quote stays where it is: once every link is inline,
/// nothing reads it.
fn definitions<'a>(root: &'a AstNode<'a>, markdown: &str, starts: &[usize]) -> Vec<Range<usize>> {
    let mut covered = vec![false; starts.len()];
    for node in root.children() {
        let data = node.data();
        for line in data.sourcepos.start.line..=data.sourcepos.end.line {
            if let Some(slot) = line.checked_sub(1).and_then(|index| covered.get_mut(index)) {
                *slot = true;
            }
        }
    }

    starts
        .iter()
        .enumerate()
        .filter(|&(index, _)| !covered.get(index).copied().unwrap_or(true))
        .map(|(index, &start)| start..starts.get(index + 1).copied().unwrap_or(markdown.len()))
        .filter(|line| !markdown[line.clone()].trim().is_empty())
        .collect()
}

#[cfg(test)]
#[path = "links_tests.rs"]
mod tests;
