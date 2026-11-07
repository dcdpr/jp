//! What fragments and highlights share: the fields a file lists in a metadata
//! block, the credits those fields name, and the token that opens an upgrade
//! section.
//!
//! A metadata block is the run of `- **Key**: value` lines that RFDs and
//! tickets carry too ([`doc::metadata`]): at the top of a fragment, or below a
//! highlight's `# Title`.
//! A value long enough to wrap continues on indented lines below its key.
//!
//! [`fields`] reads a block, [`with_credits`] writes credits into one, and
//! [`marker`] finds the lines both formats hold to the rules about upgrade
//! sections.

use std::{fmt, num::NonZeroU64};

use comrak::{Options, options::Extension};
use doc::metadata;

use crate::forge::Forge;

/// The tokens that open an upgrade section: the one Conventional Commits uses,
/// and its synonym.
const MARKERS: [&str; 2] = ["BREAKING CHANGE:", "BREAKING-CHANGE:"];

/// The field that lists who to credit in place of the ones history names.
pub(crate) const CREDITS: &str = "Credits";

/// The shortest prefix of a commit's hash a credit takes, as `git log
/// --oneline` writes one.
const SHORTEST_HASH: usize = 7;

/// The longest commit hash: a SHA-256 one.
const LONGEST_HASH: usize = 64;

/// A credit a fragment or highlight lists in place of the ones history names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListedCredit {
    /// A change request, by its number: written `#1207`, or `!1207` as GitLab
    /// writes a merge request.
    ChangeRequest(NonZeroU64),

    /// A commit, by its hash or a prefix of it at least seven characters long,
    /// lowercase.
    Commit(String),
}

impl ListedCredit {
    /// Read a credit as a file writes it.
    ///
    /// `None` for anything but `#` or `!` followed by a positive number, or a
    /// hexadecimal commit hash of 7 to 64 characters.
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

    /// The credit as a file on `forge` writes it: `!1207` on GitLab and `#1207`
    /// elsewhere, or the commit's hash.
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
    /// The credit as a project on GitHub, or on no forge, writes it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.written(Forge::GitHub))
    }
}

/// One field of a metadata block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Field {
    /// The key, as written.
    pub(crate) key: String,

    /// The value, with the lines it wraps onto joined by single spaces and the
    /// escapes a Markdown formatter adds undone.
    pub(crate) value: String,

    /// The number of the field's line in the file, counting from 1.
    pub(crate) line: usize,
}

/// The metadata block that starts at `lines[start]`, if a field opens there,
/// and the index of the first line after it.
///
/// The block is the field lines and the indented lines continuing them, up to
/// the first line that is neither, so a file without a block reads as no fields
/// that end where they start.
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

/// The credit `field`, a `Credits` field, names; `None` when its value names
/// none.
pub(crate) fn credit(field: &Field) -> Option<ListedCredit> {
    ListedCredit::parse(&field.value)
}

/// What an invalid `Credits` field says, for the errors of both formats.
pub(crate) fn invalid_credit(line: usize, value: &str) -> String {
    format!(
        "line {line} credits `{value}`, which is neither a change request, such as `#1207`, nor a \
         commit's hash"
    )
}

/// `source` with its byte order mark dropped and every `\r\n` read as `\n`.
pub(crate) fn normalize(source: &str) -> String {
    source
        .strip_prefix('\u{feff}')
        .unwrap_or(source)
        .replace("\r\n", "\n")
}

/// Whether `line` is a level-one heading, `# Title`, the line a highlight opens
/// with.
pub(crate) fn is_title(line: &str) -> bool {
    line.trim_start()
        .strip_prefix('#')
        .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t']))
}

/// `source`, a fragment or, with `highlight`, a highlight, with `credits`
/// listed in its metadata block as `Credits` fields, each as a file on `forge`
/// writes it.
///
/// The fields go at the end of the block, which is at the top of a fragment and
/// below a highlight's title; a file without a block gets one there.
/// The rest of the file is kept as it is, line endings and byte order mark
/// included.
/// The file must not list credits already, since it would then list both.
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

    let has_text = |index: &usize| !plain[*index].trim().is_empty();
    let start = match (0..plain.len()).find(has_text) {
        Some(title) if highlight && is_title(plain[title]) => (title + 1..plain.len())
            .find(has_text)
            .unwrap_or(plain.len()),
        Some(first) => first,
        None => plain.len(),
    };

    let mut block = String::new();
    for credit in credits {
        block.push_str(&metadata::line(CREDITS, &credit.written(forge)));
        block.push_str(newline);
    }
    let (existing, end) = fields(&plain, start);
    let at = if existing.is_empty() {
        // A new block, set off from the text after it.
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

/// The token that opens an upgrade section `line` starts with, if it starts
/// with one.
pub(crate) fn marker(line: &str) -> Option<&'static str> {
    MARKERS.into_iter().find(|marker| line.starts_with(marker))
}

/// Parse options that read a body as GitHub renders it, tables included: a
/// table can take a line that would otherwise start a paragraph.
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
