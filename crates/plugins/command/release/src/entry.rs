//! Fragment and highlight file paths, slug derivation, and fragment writing.
//!
//! Fragments live at `<fragments_dir>/<slug>.<type>.md` and highlights live at
//! `<fragments_dir>/highlights/<slug>.md`.
//! [`plan`] resolves the target path, applies `.comfort.toml` formatting when
//! `format_on_write` is enabled, and validates the fragment both before and
//! after formatting.
//! [`Planned::write`] writes the validated file to disk.
//!
//! When no explicit slug is provided, one is derived from the plain text of the
//! entry, the highlight's `# Title`, or the current git branch name.

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

pub(crate) const HIGHLIGHTS: &str = "highlights";

/// Maximum length in bytes of a slug derived from an entry or title.
const MAX_DERIVED_SLUG: usize = 50;

/// Workspace fragments directory and its Markdown formatting policy.
#[derive(Debug, Clone)]
pub(crate) struct Fragments<'a> {
    pub root: &'a Utf8Path,
    pub dir: Utf8PathBuf,
    pub formatting: Formatting,
}

impl<'a> Fragments<'a> {
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
    /// Format `path` relative to the workspace root using `/` separators.
    pub(crate) fn show(&self, path: &Utf8Path) -> String {
        path.strip_prefix(self.root)
            .unwrap_or(path)
            .as_str()
            .replace(MAIN_SEPARATOR, "/")
    }

    pub(crate) fn fragment(&self, slug: &str, type_: Type) -> Utf8PathBuf {
        self.dir.join(fragment::file_name(slug, type_))
    }

    pub(crate) fn highlight(&self, slug: &str) -> Utf8PathBuf {
        self.dir.join(HIGHLIGHTS).join(format!("{slug}.md"))
    }
}

/// Validated fragment or highlight ready to be written to disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Planned {
    pub path: Utf8PathBuf,
    pub shown: String,
    pub text: String,
}

pub(crate) fn ready(fragments: &Fragments<'_>) -> Result<(), String> {
    if fragments.dir.is_dir() {
        return Ok(());
    }

    Err(format!(
        "There is no fragments directory at {}; create it with `jp release init`.",
        fragments.show(&fragments.dir)
    ))
}

/// Validate and format a fragment of `type_` with `body`, resolving its target
/// path from `slug` (or deriving a slug from the entry's first words).
///
/// Validates `body` both before and after applying `.comfort.toml` formatting,
/// because sentence-per-line reflow can move a mid-paragraph `BREAKING CHANGE:`
/// token to the start of a line.
/// When `slug` is derived, existing collisions receive a `-2`, `-3`, ...
/// suffix; when `slug` is explicit, an existing file is rejected.
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
    /// Atomically create and write the planned file, failing if the file was
    /// created since [`plan`] ran.
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

/// Validate that `slug` is non-empty and contains only ASCII alphanumerics,
/// `-`, and `_`.
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

/// Derive a slug from the last `/`-separated segment of `branch` (e.g.
/// `fix-idle-turns` from `jean/fix-idle-turns`), returning `None` for `main` or
/// branches with no ASCII word characters.
pub(crate) fn branch_slug(branch: &str) -> Option<String> {
    if branch == MAIN {
        return None;
    }

    derive_slug(branch.rsplit('/').next().unwrap_or(branch))
}

/// Extract plain text and inline code from `markdown`, replacing line and block
/// breaks with spaces and stripping link URLs.
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

/// Derive a lowercase `-`-joined slug of at most [`MAX_DERIVED_SLUG`] bytes
/// from the leading ASCII alphanumeric words of `text`.
pub(crate) fn derive_slug(text: &str) -> Option<String> {
    let mut slug = String::new();
    let words = text
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty());

    for word in words {
        let word = word.to_ascii_lowercase();
        if slug.is_empty() {
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

/// Return the first unoccupied path in `dir` among `name(slug)`,
/// `name("<slug>-2")`, `name("<slug>-3")`, and so on.
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
