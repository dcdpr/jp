//! Rewriting what other documents say about an RFD when its id or its file name
//! changes, and finding what is left for someone to fix by hand.
//!
//! A change is a list of [`Rule`]s applied to a document's text in order.
//! [`rewrite_file`] applies them to one file and writes it back only when it
//! changed; [`mentions`] walks a directory the way `rg` does and names the
//! files a rewrite didn't reach that still mention the old id.

use std::{fs, io};

use camino::{Utf8Path, Utf8PathBuf};
use ignore::WalkBuilder;

/// One substitution over a document's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Rule {
    /// `RFD <old>` where no digit follows: `RFD 042` but not `RFD 0420`.
    Citation { old: String, new: String },

    /// A link target: the file stem `stem` (`D05-slug`, or a path ending in
    /// one) where no slug character follows, and `.md` after it if there is
    /// one, which is kept.
    Stem { stem: String, to: String },

    /// A bare id with something other than a letter, digit, or `_` on both
    /// sides: `D05` in `D05,` but not in `D056` or `xD05`.
    Token { old: String, new: String },
}

impl Rule {
    fn apply(&self, text: &str) -> String {
        match self {
            Self::Citation { old, new } => replace_where(
                text,
                &format!("RFD {old}"),
                &format!("RFD {new}"),
                |_, after| !after.is_some_and(|c| c.is_ascii_digit()),
            ),
            Self::Stem { stem, to } => {
                replace_where(text, stem, to, |_, after| !after.is_some_and(is_slug))
            }
            Self::Token { old, new } => replace_where(text, old, new, |before, after| {
                !before.is_some_and(is_word) && !after.is_some_and(is_word)
            }),
        }
    }
}

/// Whether `c` continues a word, the way a regex `\w` reads ASCII text.
fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Whether `c` continues a file name's slug.
fn is_slug(c: char) -> bool {
    is_word(c) || c == '-'
}

/// Replace each occurrence of `from` that `keep` accepts, given the characters
/// right before and after it.
fn replace_where(
    text: &str,
    from: &str,
    to: &str,
    keep: impl Fn(Option<char>, Option<char>) -> bool,
) -> String {
    if from.is_empty() {
        return text.to_owned();
    }

    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut before = None;
    while let Some(at) = rest.find(from) {
        let (head, tail) = rest.split_at(at);
        let preceding = head.chars().next_back().or(before);
        let following = tail[from.len()..].chars().next();

        out.push_str(head);
        if keep(preceding, following) {
            out.push_str(to);
        } else {
            out.push_str(from);
        }

        before = from.chars().next_back();
        rest = &tail[from.len()..];
    }
    out.push_str(rest);

    out
}

/// Apply `rules` to `text`, in order.
#[must_use]
pub(crate) fn apply(text: &str, rules: &[Rule]) -> String {
    rules
        .iter()
        .fold(text.to_owned(), |text, rule| rule.apply(&text))
}

/// Whether `text` cites `RFD <id>` where no letter, digit, or `_` follows.
#[must_use]
pub(crate) fn cites(text: &str, id: &str) -> bool {
    let citation = format!("RFD {id}");
    text.match_indices(&citation).any(|(at, _)| {
        !text[at + citation.len()..]
            .chars()
            .next()
            .is_some_and(is_word)
    })
}

/// Whether `text` names the file stem `stem` where no slug character follows:
/// `042-slug` in `042-slug.md` and in `(042-slug)`, but not in `042-slug-two`.
#[must_use]
pub(crate) fn names_stem(text: &str, stem: &str) -> bool {
    text.match_indices(stem)
        .any(|(at, _)| !text[at + stem.len()..].chars().next().is_some_and(is_slug))
}

/// The markdown files directly in `dir`, by name.
///
/// A directory that doesn't exist has none.
pub(crate) fn markdown_files(dir: &Utf8Path) -> Result<Vec<Utf8PathBuf>, String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(format!("{dir}: {error}")),
    };

    let mut files = vec![];
    for entry in entries {
        let entry = entry.map_err(|error| format!("{dir}: {error}"))?;
        let Ok(path) = Utf8PathBuf::try_from(entry.path()) else {
            continue;
        };
        if path.extension() == Some("md") && path.is_file() {
            files.push(path);
        }
    }
    files.sort();

    Ok(files)
}

/// Rewrite `file` with `rules`, returning whether it changed.
///
/// A file that isn't text is left alone.
pub(crate) fn rewrite_file(file: &Utf8Path, rules: &[Rule]) -> Result<bool, String> {
    let source = match fs::read_to_string(file) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::InvalidData => return Ok(false),
        Err(error) => return Err(format!("{file}: {error}")),
    };

    let rewritten = apply(&source, rules);
    if rewritten == source {
        return Ok(false);
    }

    fs::write(file, rewritten).map_err(|error| format!("{file}: {error}"))?;

    Ok(true)
}

/// Every file under `dir` a search looks at, sorted.
///
/// Hidden files and what `.gitignore` and `.ignore` exclude are passed over,
/// the way `rg` passes over them.
#[must_use]
pub(crate) fn walk(dir: &Utf8Path) -> Vec<Utf8PathBuf> {
    let mut files: Vec<Utf8PathBuf> = WalkBuilder::new(dir)
        .standard_filters(true)
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter_map(|entry| Utf8PathBuf::try_from(entry.into_path()).ok())
        .collect();
    files.sort();

    files
}

/// The files under `dir`, outside every directory in `excluded`, whose text
/// `matches` accepts.
///
/// Files that aren't text are passed over.
#[must_use]
pub(crate) fn mentions(
    dir: &Utf8Path,
    excluded: &[&Utf8Path],
    matches: impl Fn(&str) -> bool,
) -> Vec<Utf8PathBuf> {
    walk(dir)
        .into_iter()
        .filter(|file| !excluded.iter().any(|skip| file.starts_with(skip)))
        .filter(|file| fs::read_to_string(file).is_ok_and(|text| matches(&text)))
        .collect()
}

/// The path from the directory `from` to `to`, both under one root, as a link
/// writes it: `../rfd` from `docs/ticket` to `docs/rfd`, and empty from a
/// directory to itself.
///
/// Separators are always `/`, whatever the platform: the path goes into
/// markdown.
#[must_use]
pub(crate) fn relative_path(from: &Utf8Path, to: &Utf8Path) -> String {
    let from: Vec<&str> = from.components().map(|part| part.as_str()).collect();
    let to: Vec<&str> = to.components().map(|part| part.as_str()).collect();
    let shared = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();

    let up = from[shared..].iter().map(|_| "..");
    let down = to[shared..].iter().copied();

    up.chain(down).collect::<Vec<_>>().join("/")
}

/// A link from a document in `from` to `file` in `to`: the file name alone when
/// both sit in one directory.
#[must_use]
pub(crate) fn link(from: &Utf8Path, to: &Utf8Path, file: &str) -> String {
    let path = relative_path(from, to);
    if path.is_empty() {
        file.to_owned()
    } else {
        format!("{path}/{file}")
    }
}

#[cfg(test)]
#[path = "references_tests.rs"]
mod tests;
