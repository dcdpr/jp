//! Shared parsing helpers for fragments and highlights: `- **Key**: value`
//! metadata blocks ([`doc::metadata`]), `Credits` overrides, and `BREAKING
//! CHANGE:` markers.

use std::{fmt, num::NonZeroU64};

use comrak::{Options, options::Extension};
use doc::metadata;

use crate::forge::Forge;

/// Prefixes that open an upgrade section in a fragment.
const MARKERS: [&str; 2] = ["BREAKING CHANGE:", "BREAKING-CHANGE:"];

pub(crate) const CREDITS: &str = "Credits";
const SHORTEST_HASH: usize = 7;
const LONGEST_HASH: usize = 64;

/// Explicit attribution entry from a `- **Credits**:` metadata field,
/// overriding git history attribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListedCredit {
    /// Pull request (`#1207`) or GitLab merge request (`!1207`) number.
    ChangeRequest(NonZeroU64),

    /// Lowercase hexadecimal commit hash (7 to 64 characters).
    Commit(String),
}

impl ListedCredit {
    /// Parse a credit value (`#1207`, `!1207`, or a 7 to 64 character hex
    /// commit hash).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if let Some(number) = text.strip_prefix(['#', '!']) {
            return number.parse().ok().map(Self::ChangeRequest);
        }

        let hash = (SHORTEST_HASH..=LONGEST_HASH).contains(&text.len())
            && text.bytes().all(|byte| byte.is_ascii_hexdigit());
        hash.then(|| Self::Commit(text.to_ascii_lowercase()))
    }

    /// Format the credit for `forge` (`!1207` on GitLab, `#1207` elsewhere, or
    /// the commit hash).
    #[must_use]
    pub fn written(&self, forge: Forge) -> String {
        match (self, forge) {
            (Self::ChangeRequest(number), Forge::GitLab) => format!("!{number}"),
            (Self::ChangeRequest(number), _) => format!("#{number}"),
            (Self::Commit(hash), _) => hash.clone(),
        }
    }
}

impl fmt::Display for ListedCredit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.written(Forge::GitHub))
    }
}

/// Parsed metadata field from a `- **Key**: value` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Field {
    pub(crate) key: String,

    /// Field value with wrapped continuation lines collapsed and Markdown
    /// escapes unescaped.
    pub(crate) value: String,

    /// 1-based line number where the field starts.
    pub(crate) line: usize,
}

/// Parse a contiguous `- **Key**: value` metadata block starting at
/// `lines[start]`, including indented continuation lines, and return the parsed
/// fields alongside the index of the first line after the block.
pub(crate) fn fields(lines: &[&str], start: usize) -> (Vec<Field>, usize) {
    let mut fields: Vec<Field> = vec![];
    let mut index = start;

    while let Some(line) = lines.get(index) {
        if let Some((key, value)) = metadata::split(line) {
            fields.push(Field {
                key: key.to_owned(),
                value: value.to_owned(),
                line: index + 1,
            });
        } else if let Some(field) = fields.last_mut()
            && line.starts_with([' ', '\t'])
            && !line.trim().is_empty()
        {
            field.value.push(' ');
            field.value.push_str(line.trim());
        } else {
            break;
        }
        index += 1;
    }

    for field in &mut fields {
        let joined = field.value.split_whitespace().collect::<Vec<_>>().join(" ");
        field.value = metadata::unescape(&joined);
    }

    (fields, index)
}

pub(crate) fn credit(field: &Field) -> Option<ListedCredit> {
    ListedCredit::parse(&field.value)
}

pub(crate) fn invalid_credit(line: usize, value: &str) -> String {
    format!(
        "line {line} credits `{value}`, which is neither a change request, such as `#1207`, nor a \
         commit's hash"
    )
}

/// Strip a leading UTF-8 BOM and normalize `\r\n` line endings to `\n`.
pub(crate) fn normalize(source: &str) -> String {
    source
        .strip_prefix('\u{feff}')
        .unwrap_or(source)
        .replace("\r\n", "\n")
}

/// Return whether `line` is an ATX level-1 heading (`# ...`).
pub(crate) fn is_title(line: &str) -> bool {
    line.trim_start()
        .strip_prefix('#')
        .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t']))
}

/// Insert `credits` as `- **Credits**:` metadata fields into `source` (a
/// fragment, or a highlight when `highlight` is true), formatted for `forge`.
///
/// Appends to the existing metadata block (at the top of a fragment or below
/// the `# Title` of a highlight), or creates a new block if none exists,
/// preserving the original BOM and line endings.
/// Assumes `source` does not already contain `Credits` fields.
#[must_use]
pub fn with_credits(
    source: &str,
    highlight: bool,
    credits: &[ListedCredit],
    forge: Forge,
) -> String {
    if credits.is_empty() {
        return source.to_owned();
    }

    let text = source.strip_prefix('\u{feff}').unwrap_or(source);
    let bom = &source[..source.len() - text.len()];
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let plain: Vec<&str> = lines
        .iter()
        .map(|line| line.trim_end_matches(['\r', '\n']))
        .collect();

    let start = block_start(&plain, highlight);

    let mut block = String::new();
    for credit in credits {
        block.push_str(&metadata::line(CREDITS, &credit.written(forge)));
        block.push_str(newline);
    }
    let (existing, end) = fields(&plain, start);
    let at = if existing.is_empty() {
        if start < plain.len() {
            block.push_str(newline);
        }
        start
    } else {
        end
    };

    let mut written = bom.to_owned();
    for line in &lines[..at] {
        written.push_str(line);
    }
    if at > 0 && !lines[at - 1].ends_with('\n') {
        written.push_str(newline);
    }
    written.push_str(&block);
    for line in &lines[at..] {
        written.push_str(line);
    }

    written
}

/// Return the line index where the metadata block starts (or would be
/// inserted): the first non-empty line for a fragment, or the first non-empty
/// line after the `# Title` heading for a highlight.
fn block_start(plain: &[&str], highlight: bool) -> usize {
    let has_text = |index: &usize| !plain[*index].trim().is_empty();

    match (0..plain.len()).find(has_text) {
        Some(title) if highlight && is_title(plain[title]) => (title + 1..plain.len())
            .find(has_text)
            .unwrap_or(plain.len()),
        Some(first) => first,
        None => plain.len(),
    }
}

/// Return the `BREAKING CHANGE:` or `BREAKING-CHANGE:` prefix if `line` starts
/// with one.
pub(crate) fn marker(line: &str) -> Option<&'static str> {
    MARKERS.into_iter().find(|marker| line.starts_with(marker))
}

/// Comrak options with GFM table parsing enabled so table rows cannot be
/// misparsed as paragraph starts.
pub(crate) fn markdown_options() -> Options<'static> {
    Options {
        extension: Extension {
            table: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[cfg(test)]
#[path = "document_tests.rs"]
mod tests;
