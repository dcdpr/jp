//! Rewriting reference-style Markdown links (`[text][label]`, `[label][]`,
//! `[label]`) to inline links (`[text](url)`).
//!
//! Reference link definitions (`[label]: url`) often sit at the bottom of a
//! file below a fragment's upgrade section, and two fragments rendered into the
//! same document may define the same label with different URLs.
//! [`Links::read`] locates all reference links, images, and link definitions in
//! a Markdown document, and [`Links::standalone`] renders any byte range with
//! its links inlined and top-level definitions stripped.

use std::{iter, ops::Range};

use comrak::{
    Arena,
    nodes::{AstNode, NodeValue, Sourcepos},
    parse_document,
};

use crate::document;

/// Reference-link rewrites and link-definition removals for a Markdown
/// document.
pub(crate) struct Links {
    /// Non-overlapping `(byte_range, replacement)` edits sorted by start
    /// offset.
    edits: Vec<(Range<usize>, String)>,
}

impl Links {
    /// Locate all reference-style links, reference images, and link definitions
    /// in `markdown`.
    ///
    /// Top-level link definitions are stripped only when every link in the
    /// document was classified, so any link left untouched retains its
    /// definition.
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

    /// Return `markdown[range]` with reference links rewritten to inline links
    /// and top-level link definitions removed.
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

/// Return the starting byte offset of each line in `text`.
pub(crate) fn line_starts(text: &str) -> Vec<usize> {
    iter::once(0)
        .chain(text.match_indices('\n').map(|(index, _)| index + 1))
        .collect()
}

enum Form {
    /// Inline link (`[text](url)`) or autolink (`<url>`).
    Inline,

    /// Reference link, carrying the byte range of its trailing `[label]` / `[]`
    /// suffix (or an empty range at the end of `[shortcut]`) to replace with
    /// `(url)`.
    Reference(Range<usize>),
}

/// Classify the link or image at `sourcepos` in `markdown`.
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

    if !label.is_empty() && closing_bracket(label) != Some(label.len() - 1) {
        return None;
    }

    Some(Form::Reference(range.end - label.len()..range.end))
}

/// Convert comrak's 1-based inclusive `sourcepos` to a byte range in
/// `markdown`.
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

/// Find the byte index of the `]` that closes the leading `[` of `text`,
/// respecting nested brackets, backslash escapes, and inline code spans.
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

fn backticks(bytes: &[u8], index: usize) -> usize {
    bytes
        .iter()
        .skip(index)
        .take_while(|&&byte| byte == b'`')
        .count()
}

/// Find the end index of the first backtick run of length `count` at or after
/// `from`.
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

/// Format an inline link destination and optional title as `(url)` or `(url
/// "title")`.
fn inline(url: &str, title: &str) -> String {
    let destination = if bare(url) {
        url.to_owned()
    } else {
        format!("<{}>", escaped(url, &['\\', '<', '>']))
    };
    if title.is_empty() {
        return format!("({destination})");
    }

    let title = escaped(&title.replace('\n', " "), &['\\', '"']);
    format!("({destination} \"{title}\")")
}

/// Return whether `url` can be written as a bare CommonMark link destination
/// without `<...>` delimiters.
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

/// Return the byte ranges of top-level link reference definitions in
/// `markdown`.
///
/// Comrak consumes link reference definitions without emitting AST nodes, so
/// any non-empty line not covered by a top-level AST child is a reference
/// definition.
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
