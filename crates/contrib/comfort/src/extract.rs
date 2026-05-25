//! Doc-comment block extraction from Rust source.
//!
//! A *block* is a maximal run of consecutive line doc-comments — either outer
//! (`///`) or inner (`//!`) — sharing the same indentation and separated only
//! by a single newline.
//! Blank lines inside the block (i.e.
//! `///\n` with no body content) are part of the block; a truly blank source
//! line ends it.

use std::ops::Range;

use ra_ap_rustc_lexer::{DocStyle, FrontmatterAllowed, TokenKind, tokenize};

/// A contiguous run of `///` or `//!` lines in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// Byte range covering the block in the original source, starting at the
    /// indent of the first line and ending just past the last comment line's
    /// last character (not including its trailing newline).
    pub range: Range<usize>,

    /// Whitespace prefix shared by every line of the block.
    pub indent: String,

    /// Outer (`///`) or inner (`//!`).
    pub style: DocStyle,

    /// Markdown body, one entry per source line, with the prefix and at most
    /// one separator space stripped.
    /// Empty entries represent blank doc-comment lines (a `///` with nothing
    /// after it).
    pub lines: Vec<String>,
}

impl Block {
    /// The marker for this block's style: `///` or `//!`.
    #[must_use]
    pub fn marker(&self) -> &'static str {
        match self.style {
            DocStyle::Outer => "///",
            DocStyle::Inner => "//!",
        }
    }

    /// Source-column overhead of this block's per-line prefix: indent, marker,
    /// and separator space.
    /// Used to compute how much of a global `max_width` budget is left for body
    /// content.
    /// Counted in bytes; for the all-ASCII whitespace indents Rust source uses,
    /// this matches the rendered column count.
    #[must_use]
    pub fn prefix_width(&self) -> usize {
        self.indent.len() + self.marker().len() + 1
    }

    /// Reassemble the block as Rust source from a freshly formatted markdown
    /// body.
    /// Lines are split on `\n`; non-empty lines get ` {indent}{marker}  `,
    /// empty lines get `{indent}{marker}` with no trailing space.
    #[must_use]
    pub fn reassemble(&self, formatted_body: &str) -> String {
        let marker = self.marker();
        let mut out = String::with_capacity(formatted_body.len() + self.indent.len() * 4);

        for (i, line) in formatted_body.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&self.indent);
            out.push_str(marker);
            if !line.is_empty() {
                out.push(' ');
                out.push_str(line);
            }
        }

        out
    }
}

/// Find all outer/inner doc-comment blocks in `source`, in source order.
///
/// Only line doc-comments are recognised.
/// Block doc-comments (`/** */`, `/*! */`) and regular `//` comments are
/// ignored.
/// Doc-comments that don't start at the line's first non-whitespace character
/// (e.g. a `///` trailing some code) are also skipped.
#[must_use]
pub fn find_blocks(source: &str) -> Vec<Block> {
    let bytes = source.as_bytes();
    let mut blocks: Vec<Block> = Vec::new();
    let mut offset: usize = 0;

    // Pending block we're still extending across consecutive lines.
    let mut pending: Option<PendingBlock> = None;

    for token in tokenize(source, FrontmatterAllowed::Yes) {
        let token_start = offset;
        let token_end = offset + token.len as usize;
        offset = token_end;

        match token.kind {
            TokenKind::LineComment {
                doc_style: Some(style),
            } => {
                // Confirm the comment starts at the beginning of a logical line
                // (only whitespace between the previous '\n' and this token).
                let line_start = line_start_of(bytes, token_start);
                let leading = &source[line_start..token_start];
                if !leading.chars().all(|c| c == ' ' || c == '\t') {
                    // Trailing comment after code; flush any pending block.
                    if let Some(prev) = pending.take() {
                        blocks.push(prev.into_block());
                    }
                    continue;
                }

                let body = extract_body(&source[token_start..token_end], style);

                match pending.as_mut() {
                    Some(prev)
                        if prev.style == style
                            && prev.indent == leading
                            && prev.next_line_start == line_start =>
                    {
                        prev.lines.push(body);
                        prev.end = token_end;
                    }
                    _ => {
                        if let Some(prev) = pending.take() {
                            blocks.push(prev.into_block());
                        }
                        pending = Some(PendingBlock {
                            start: line_start,
                            end: token_end,
                            indent: leading.to_owned(),
                            style,
                            lines: vec![body],
                            next_line_start: line_start,
                        });
                    }
                }
            }
            TokenKind::Whitespace => {
                // The block extends across a single `\n`. Two or more newlines
                // (a truly blank source line) break the block.
                let Some(prev) = pending.as_mut() else {
                    continue;
                };
                let ws = &source[token_start..token_end];
                let mut newline_idx = None;
                let mut newlines = 0_usize;
                for (i, b) in ws.bytes().enumerate() {
                    if b == b'\n' {
                        newlines += 1;
                        newline_idx = Some(i);
                    }
                }
                if newlines == 1 {
                    // Predict where the next line starts so we can confirm
                    // the next comment is at column 0 of that line.
                    let idx = newline_idx.unwrap_or(0);
                    prev.next_line_start = token_start + idx + 1;
                } else if let Some(prev) = pending.take() {
                    blocks.push(prev.into_block());
                }
            }
            _ => {
                if let Some(prev) = pending.take() {
                    blocks.push(prev.into_block());
                }
            }
        }
    }

    if let Some(prev) = pending {
        blocks.push(prev.into_block());
    }

    blocks
}

struct PendingBlock {
    start: usize,
    end: usize,
    indent: String,
    style: DocStyle,
    lines: Vec<String>,
    // Byte position where the next line begins, used to confirm that the
    // following doc-comment (if any) is the first content on its line.
    next_line_start: usize,
}

impl PendingBlock {
    fn into_block(self) -> Block {
        Block {
            range: self.start..self.end,
            indent: self.indent,
            style: self.style,
            lines: self.lines,
        }
    }
}

/// Strip the `///` / `//!` marker and an optional single separator space.
fn extract_body(raw: &str, style: DocStyle) -> String {
    let marker = match style {
        DocStyle::Outer => "///",
        DocStyle::Inner => "//!",
    };
    let rest = raw.strip_prefix(marker).unwrap_or(raw);
    // Strip at most one separator space; preserve additional indentation so
    // markdown code blocks indented by 4+ spaces survive the round-trip.
    rest.strip_prefix(' ').unwrap_or(rest).to_owned()
}

/// Walk backwards from `pos` to find the byte index just past the previous `\n`
/// (or 0 if there is none).
fn line_start_of(bytes: &[u8], pos: usize) -> usize {
    bytes[..pos]
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |i| i + 1)
}

#[cfg(test)]
#[path = "extract_tests.rs"]
mod tests;
