//! Formatting documents as they are written, in the style `comfort` reads from
//! the workspace's `.comfort.toml`.
//!
//! A workspace turns it on per kind of document, in the options of the plugin
//! that keeps them:
//!
//! ```toml
//! [plugins.command.ticket.options]
//! format_on_write = true
//! ```
//!
//! The setting belongs to the documents rather than to the plugin writing them:
//! `jp wiki` rewriting a link inside an RFD formats the RFD when, and only
//! when, `jp rfd`'s `format_on_write` is on.
//! A file outside every document directory is never formatted.

use std::{borrow::Cow, fs, io};

use camino::{Utf8Path, Utf8PathBuf};
use comfort::{
    config::Config,
    format::{FormatOptions, format_markdown_with},
};
use serde_json::Value;

/// The option, under a plugin's `options`, that turns formatting on for the
/// documents it keeps.
pub const FORMAT_ON_WRITE: &str = "format_on_write";

/// Which files a write formats, and the style it formats them in.
///
/// The default formats nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Formatting {
    style: FormatOptions,

    /// The directories whose markdown files are formatted, each holding one
    /// kind of document.
    dirs: Vec<Utf8PathBuf>,
}

impl Formatting {
    /// Format the markdown files in `dirs` in `style`.
    #[must_use]
    pub fn new(style: FormatOptions, dirs: Vec<Utf8PathBuf>) -> Self {
        Self { style, dirs }
    }

    /// The formatting the workspace at `root` configures for `kinds`, each the
    /// name of the plugin keeping a kind of document and the directory it keeps
    /// them in.
    ///
    /// `config` is the whole configuration the host passed in `init`.
    /// `.comfort.toml` at `root` is read only when some kind turns formatting
    /// on; without one, comfort's defaults apply.
    ///
    /// # Errors
    ///
    /// Returns a message naming the setting or the file that is wrong.
    pub fn configure(
        root: &Utf8Path,
        config: &Value,
        kinds: &[(&str, &Utf8Path)],
    ) -> Result<Self, String> {
        let mut dirs = vec![];
        for (plugin, dir) in kinds {
            if format_on_write(config, plugin)? {
                dirs.push(dir.to_path_buf());
            }
        }

        if dirs.is_empty() {
            return Ok(Self::default());
        }

        let style = Config::read_in(root.as_std_path())
            .map_err(|error| error.to_string())?
            .format_options();

        Ok(Self::new(style, dirs))
    }

    /// Whether a write to `path` formats it: a markdown file inside one of the
    /// directories formatting is on for.
    #[must_use]
    pub fn formats(&self, path: &Utf8Path) -> bool {
        path.extension() == Some("md") && self.dirs.iter().any(|dir| path.starts_with(dir))
    }

    /// `text` the way a write to `path` lays it out: formatted when
    /// [`Formatting::formats`] says so, as given otherwise.
    #[must_use]
    pub fn apply<'a>(&self, path: &Utf8Path, text: &'a str) -> Cow<'a, str> {
        if self.formats(path) {
            Cow::Owned(format_markdown_with(text, &self.style))
        } else {
            Cow::Borrowed(text)
        }
    }

    /// Write `text` to `path`, formatted when [`Formatting::formats`] says so.
    ///
    /// # Errors
    ///
    /// Returns the I/O error of a file that can't be written.
    pub fn write(&self, path: &Utf8Path, text: &str) -> io::Result<()> {
        fs::write(path, self.apply(path, text).as_ref())
    }
}

/// Whether the plugin `plugin` formats the documents it keeps as they are
/// written: its `format_on_write` option, off when unset.
///
/// `config` is the whole configuration the host passed in `init`.
///
/// # Errors
///
/// Returns a message naming the setting when it isn't `true` or `false`.
pub fn format_on_write(config: &Value, plugin: &str) -> Result<bool, String> {
    match config.pointer(&format!(
        "/plugins/command/{plugin}/options/{FORMAT_ON_WRITE}"
    )) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(on)) => Ok(*on),
        Some(_) => Err(format!(
            "`plugins.command.{plugin}.options.{FORMAT_ON_WRITE}` must be `true` or `false`."
        )),
    }
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
