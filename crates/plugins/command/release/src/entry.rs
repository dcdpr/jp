//! Where fragments and highlights are filed, and writing a fragment the
//! assistant gives.
//!
//! A fragment is `<slug>.<type>.md` in the fragments directory, and a highlight
//! is `<slug>.md` in its `highlights/`.
//! [`plan`] settles the file a fragment is written to, lays it out as the
//! workspace formats the fragments directory, and holds it to the format `jp
//! release check` holds it to, as it is laid out; [`Planned::write`] writes it.
//!
//! A slug the caller doesn't give is derived from the first words of the entry,
//! of a highlight's title, or of the git branch's name, as they read without
//! their markup and link URLs.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write as _},
    path::MAIN_SEPARATOR,
};

use camino::{Utf8Path, Utf8PathBuf};
use changelog::{Type, fragment};
use comrak::{Arena, Options, nodes::NodeValue, parse_document};
use doc::format::Formatting;
use serde_json::Value;

use crate::{PLUGIN, repository::MAIN};

/// The directory, inside the fragments directory, that holds highlights.
pub(crate) const HIGHLIGHTS: &str = "highlights";

/// The longest slug derived from an entry, in bytes: room for the first few
/// words, which is what tells one file from another in a listing.
const MAX_DERIVED_SLUG: usize = 50;

/// The fragments directory of a workspace.
#[derive(Debug, Clone)]
pub(crate) struct Fragments<'a> {
    /// The workspace root, which paths in messages are relative to.
    pub root: &'a Utf8Path,

    /// The fragments directory.
    pub dir: Utf8PathBuf,

    /// How a file written in the fragments directory is laid out.
    pub formatting: Formatting,
}

impl<'a> Fragments<'a> {
    /// The fragments directory `dir` of the workspace at `root`, laying files
    /// out as `config`, the whole configuration the host passed in `init`,
    /// says: in the style of the workspace's `.comfort.toml` when
    /// `format_on_write` is on, as they are given otherwise.
    ///
    /// The error names the setting or the file that is wrong.
    pub(crate) fn new(
        root: &'a Utf8Path,
        dir: Utf8PathBuf,
        config: &Value,
    ) -> Result<Self, String> {
        let formatting = Formatting::configure(root, config, &[(PLUGIN, &dir)])?;

        Ok(Self {
            root,
            dir,
            formatting,
        })
    }
}

impl Fragments<'_> {
    /// `path` as a message names it: relative to the workspace root, with `/`
    /// between its parts on every platform.
    pub(crate) fn show(&self, path: &Utf8Path) -> String {
        path.strip_prefix(self.root)
            .unwrap_or(path)
            .as_str()
            .replace(MAIN_SEPARATOR, "/")
    }

    /// The fragment of type `type_` named `slug`: `<slug>.<type>.md`.
    pub(crate) fn fragment(&self, slug: &str, type_: Type) -> Utf8PathBuf {
        self.dir.join(fragment::file_name(slug, type_))
    }

    /// The highlight named `slug`: `highlights/<slug>.md`.
    pub(crate) fn highlight(&self, slug: &str) -> Utf8PathBuf {
        self.dir.join(HIGHLIGHTS).join(format!("{slug}.md"))
    }
}

/// A file about to be written: where it goes, and what it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Planned {
    pub path: Utf8PathBuf,

    /// [`Planned::path`] as messages name it.
    pub shown: String,

    pub text: String,
}

/// Check that `fragments` is there to take a new file.
pub(crate) fn ready(fragments: &Fragments<'_>) -> Result<(), String> {
    if fragments.dir.is_dir() {
        return Ok(());
    }

    Err(format!(
        "There is no fragments directory at {}; create it with `jp release init`.",
        fragments.show(&fragments.dir)
    ))
}

/// Settle the file in `fragments` that the fragment of type `type_` holding
/// `body` is written to, named by `slug` or by one derived from the entry, and
/// lay it out as the workspace formats the fragments directory.
///
/// The fragment is held to its format as given and again as laid out, since
/// laying a paragraph out a sentence per line can start a line with an upgrade
/// section's token.
/// A derived slug a file of the type already has gets `-2`, `-3`, and so on,
/// while a `slug` the caller named is refused instead.
/// The error says what to change.
pub(crate) fn plan(
    fragments: &Fragments<'_>,
    type_: Type,
    body: &str,
    slug: Option<&str>,
) -> Result<Planned, String> {
    ready(fragments)?;
    if let Some(slug) = slug {
        check_slug(slug)?;
    }

    let given = format!("{}\n", body.trim());
    let parsed = fragment::parse(type_, &given).map_err(|error| error.to_string())?;

    let path = if let Some(slug) = slug {
        let path = fragments.fragment(slug, type_);
        if path.exists() {
            return Err(format!(
                "{} already exists; choose another slug.",
                fragments.show(&path)
            ));
        }
        path
    } else {
        let slug = derive_slug(&plain_text(&parsed.entry))
            .ok_or("Nothing in the entry to name its file after; give it a slug.")?;
        unused(&fragments.dir, &slug, |slug| {
            fragment::file_name(slug, type_)
        })
    };

    let text = fragments.formatting.apply(&path, &given).into_owned();
    if text != given {
        fragment::parse(type_, &text)
            .map_err(|error| format!("once laid out as the workspace formats it, {error}"))?;
    }

    Ok(Planned {
        shown: fragments.show(&path),
        path,
        text,
    })
}

impl Planned {
    /// Write the file, creating its directory when it isn't there yet.
    ///
    /// Refuses to overwrite a file, including one that appeared since the file
    /// was planned.
    pub(crate) fn write(&self) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(|error| format!("failed to create {dir}: {error}"))?;
        }

        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.path)
            .and_then(|mut file| file.write_all(self.text.as_bytes()))
            .map_err(|error| match error.kind() {
                io::ErrorKind::AlreadyExists => {
                    format!("{} already exists; choose another slug.", self.shown)
                }
                _ => format!("failed to write {}: {error}", self.shown),
            })
    }
}

/// Refuse a slug that can't name a fragment or highlight: one holding more than
/// ASCII letters, digits, `-`, and `_`, which keeps it a plain file name in the
/// directory it names, and keeps a fragment's type the only `.` part of its
/// name.
pub(crate) fn check_slug(slug: &str) -> Result<(), String> {
    let plain = !slug.is_empty()
        && slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
    if plain {
        return Ok(());
    }

    Err(format!(
        "`{slug}` can't name a file here: use ASCII letters, digits, `-`, and `_`."
    ))
}

/// The slug a file is named by after the git branch `branch`: its last
/// `/`-separated part, as [`derive_slug`] reads it, so `jean/fix-idle-turns`
/// gives `fix-idle-turns`.
///
/// `None` on the default branch, which has no change of its own to name a file
/// after, and for a branch whose name holds no word.
pub(crate) fn branch_slug(branch: &str) -> Option<String> {
    if branch == MAIN {
        return None;
    }

    derive_slug(branch.rsplit('/').next().unwrap_or(branch))
}

/// What `markdown` says, without its markup: its text and inline code, with a
/// space where a line or a block breaks, and no link's URL.
pub(crate) fn plain_text(markdown: &str) -> String {
    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &Options::default());

    let mut text = String::new();
    for node in root.descendants() {
        let data = node.data();
        match &data.value {
            NodeValue::Text(literal) => text.push_str(literal),
            NodeValue::Code(code) => text.push_str(&code.literal),
            NodeValue::SoftBreak | NodeValue::LineBreak => text.push(' '),
            value if value.block() => text.push(' '),
            _ => {}
        }
    }

    text
}

/// A slug for `text`: its first words, lowercased and joined by `-`, at most
/// [`MAX_DERIVED_SLUG`] bytes.
///
/// A word is a run of ASCII letters, digits, and `_`, so Markdown between words
/// separates them like a space does.
/// `None` when `text` holds no word.
pub(crate) fn derive_slug(text: &str) -> Option<String> {
    let mut slug = String::new();
    let words = text
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty());

    for word in words {
        let word = word.to_ascii_lowercase();
        if slug.is_empty() {
            // ASCII, so any byte index is a character boundary.
            slug = word[..word.len().min(MAX_DERIVED_SLUG)].to_owned();
            continue;
        }
        if slug.len() + 1 + word.len() > MAX_DERIVED_SLUG {
            break;
        }
        slug.push('-');
        slug.push_str(&word);
    }

    (!slug.is_empty()).then_some(slug)
}

/// The first of the files `name` gives `slug`, `slug-2`, `slug-3`, and so on,
/// that `dir` doesn't hold.
pub(crate) fn unused(dir: &Utf8Path, slug: &str, name: impl Fn(&str) -> String) -> Utf8PathBuf {
    let mut path = dir.join(name(slug));
    let mut number = 2;
    while path.exists() {
        path = dir.join(name(&format!("{slug}-{number}")));
        number += 1;
    }

    path
}

#[cfg(test)]
#[path = "entry_tests.rs"]
mod tests;
