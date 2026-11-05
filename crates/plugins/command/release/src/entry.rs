//! A fragment or highlight about to be written, and writing it.
//!
//! [`Entry::render`] writes the file as it is given: the frontmatter, then the
//! body.
//! [`plan`] settles where it goes, lays it out as the workspace formats the
//! fragments directory, and holds it to the format `jp release check` holds it
//! to, as it is laid out; [`Planned::write`] writes it.
//!
//! A file is named by its slug: the one the caller gives, or one derived from
//! the first words of the entry, or of a highlight's title, as they read
//! without their markup and link URLs.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write as _},
    path::MAIN_SEPARATOR,
};

use camino::{Utf8Path, Utf8PathBuf};
use changelog::{Type, fragment, highlight, reserved_for};
use comrak::{Arena, Options, nodes::NodeValue, parse_document};
use doc::format::Formatting;
use serde_json::Value;

use crate::PLUGIN;

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
}

/// What `add` writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Entry {
    /// One user-visible change.
    Fragment { type_: Type, body: String },

    /// A section of the release notes for a feature worth a story.
    Highlight {
        title: String,
        authors: Vec<String>,
        body: String,
    },
}

/// A file about to be written: where it goes, and what it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Planned {
    pub path: Utf8PathBuf,

    /// [`Planned::path`] as messages name it.
    pub shown: String,

    pub text: String,
}

impl Entry {
    /// The file as it is given, before the workspace lays it out.
    pub(crate) fn render(&self) -> String {
        match self {
            Self::Fragment { type_, body } => {
                format!("---\ntype: {type_}\n---\n{}\n", body.trim())
            }
            Self::Highlight {
                title,
                authors,
                body,
            } => {
                let authors: Vec<String> =
                    authors.iter().map(|author| quoted(author.trim())).collect();

                format!(
                    "---\ntitle: {}\nauthors: [{}]\n---\n{}\n",
                    quoted(title.trim()),
                    authors.join(", "),
                    body.trim()
                )
            }
        }
    }

    /// Hold `text`, the entry's file as given or as laid out, to the entry's
    /// format, and derive the slug the entry names its file by when given none.
    ///
    /// The error says what in the text breaks the format.
    fn check(&self, text: &str) -> Result<Option<String>, String> {
        match self {
            Self::Fragment { .. } => fragment::parse(text)
                .map(|fragment| derive_slug(&plain_text(&fragment.entry)))
                .map_err(|error| error.to_string()),
            Self::Highlight { .. } => highlight::parse(text)
                .map(|highlight| derive_slug(&plain_text(&highlight.title)))
                .map_err(|error| error.to_string()),
        }
    }
}

/// The directory an entry is written into, inside `fragments`: the top level
/// for a fragment, `highlights/` for a highlight.
fn dir(fragments: &Fragments<'_>, highlight: bool) -> Utf8PathBuf {
    if highlight {
        fragments.dir.join(HIGHLIGHTS)
    } else {
        fragments.dir.clone()
    }
}

/// Check that `fragments` can take a new entry, named `slug` when one is given,
/// before anything is composed for it.
///
/// `highlight` says whether the entry is a highlight.
pub(crate) fn ready(
    fragments: &Fragments<'_>,
    highlight: bool,
    slug: Option<&str>,
) -> Result<(), String> {
    if !fragments.dir.is_dir() {
        return Err(format!(
            "There is no fragments directory at {}; create it with `jp release init`.",
            fragments.show(&fragments.dir)
        ));
    }

    let Some(slug) = slug else {
        return Ok(());
    };
    check_slug(slug)?;

    let path = dir(fragments, highlight).join(format!("{slug}.md"));
    if path.exists() {
        return Err(format!(
            "{} already exists; choose another slug.",
            fragments.show(&path)
        ));
    }

    Ok(())
}

/// Settle the file in `fragments` that `entry` is written to, named by `slug`
/// or by one derived from the entry, and lay it out as the workspace formats
/// the fragments directory.
///
/// The entry is held to its format as given and again as laid out, since laying
/// a paragraph out a sentence per line can start a line with an upgrade
/// section's token.
/// A derived slug a file already has gets `-2`, `-3`, and so on, while a `slug`
/// the caller named is refused instead.
/// The error says what to change.
pub(crate) fn plan(
    fragments: &Fragments<'_>,
    entry: &Entry,
    slug: Option<&str>,
) -> Result<Planned, String> {
    let highlight = matches!(entry, Entry::Highlight { .. });
    ready(fragments, highlight, slug)?;

    let given = entry.render();
    let derived = entry.check(&given)?;
    let dir = dir(fragments, highlight);

    let path = if let Some(slug) = slug {
        dir.join(format!("{slug}.md"))
    } else {
        let slug = derived.ok_or("Nothing in the entry to name its file after; give it a slug.")?;
        if let Some(reserved_for) = reserved_for(&slug) {
            return Err(format!(
                "The entry would be named `{slug}`, which is reserved for {reserved_for}; give it \
                 another slug."
            ));
        }
        unused(&dir, &slug)
    };

    let text = fragments.formatting.apply(&path, &given).into_owned();
    if text != given {
        entry.check(&text)?;
    }

    Ok(Planned {
        shown: fragments.show(&path),
        path,
        text,
    })
}

impl Planned {
    /// Write the file, creating `highlights/` when it isn't there yet.
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
/// directory it names, or a reserved one.
fn check_slug(slug: &str) -> Result<(), String> {
    let plain = !slug.is_empty()
        && slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
    if !plain {
        return Err(format!(
            "`{slug}` can't name a file here: use ASCII letters, digits, `-`, and `_`."
        ));
    }

    if let Some(reserved_for) = reserved_for(slug) {
        return Err(format!(
            "`{slug}` is reserved for {reserved_for}, in any letter case; choose another slug."
        ));
    }

    Ok(())
}

/// What `markdown` says, without its markup: its text and inline code, with a
/// space where a line or a block breaks, and no link's URL.
fn plain_text(markdown: &str) -> String {
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
fn derive_slug(text: &str) -> Option<String> {
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

/// The first of `slug.md`, `slug-2.md`, `slug-3.md`, and so on that no file in
/// `dir` has.
fn unused(dir: &Utf8Path, slug: &str) -> Utf8PathBuf {
    let mut path = dir.join(format!("{slug}.md"));
    let mut number = 2;
    while path.exists() {
        path = dir.join(format!("{slug}-{number}.md"));
        number += 1;
    }

    path
}

/// `text` as a YAML string: a JSON string literal, which YAML reads the same
/// whatever the text holds.
fn quoted(text: &str) -> String {
    Value::String(text.to_owned()).to_string()
}

#[cfg(test)]
#[path = "entry_tests.rs"]
mod tests;
