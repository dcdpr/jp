//! The style comfort formats with, read from a `.comfort.toml` file.
//!
//! ```toml
//! max_width = 80
//! format_markdown = true
//! reference_links = true
//! prune_reference_links = true
//! ```
//!
//! Every key is optional and has the same name and meaning as the command-line
//! flag it stands for.
//! A key left out takes that flag's default: an 80-column width, and every
//! transformation off.
//! A flag given on the command line wins over the file.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{DEFAULT_MAX_WIDTH, Error, format::FormatOptions};

/// The name of the configuration file.
pub const FILE_NAME: &str = ".comfort.toml";

/// A formatting style, each setting unset until a file or a flag sets it.
///
/// An unknown key is an error, so a misspelled setting is reported rather than
/// silently ignored.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Maximum line width for reflow; `0` disables width wrapping.
    pub max_width: Option<usize>,

    /// Canonicalize markdown structure: align tables, normalize list markers,
    /// prefer fenced code blocks.
    pub format_markdown: Option<bool>,

    /// Convert inline links to reference-style links collected at the end.
    pub reference_links: Option<bool>,

    /// Remove reference-link definitions nothing points at.
    pub prune_reference_links: Option<bool>,
}

impl Config {
    /// Parse the text of a configuration file.
    ///
    /// # Errors
    ///
    /// Returns the TOML error for text that isn't TOML, holds a key this module
    /// doesn't know, or holds a value of the wrong type.
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Read the configuration file at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ReadFile`] when the file can't be read and
    /// [`Error::Config`] when it doesn't parse.
    pub fn read(path: &Path) -> Result<Self, Error> {
        let text = fs::read_to_string(path).map_err(|source| Error::ReadFile {
            path: path.to_path_buf(),
            source,
        })?;

        Self::parse(&text).map_err(|source| Error::Config {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Read the configuration file directly in `dir`, or the default
    /// configuration when `dir` holds none.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Config::read`] for a file that is there.
    pub fn read_in(dir: &Path) -> Result<Self, Error> {
        let path = dir.join(FILE_NAME);
        match fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text).map_err(|source| Error::Config { path, source }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(Error::ReadFile { path, source }),
        }
    }

    /// The configuration file in `start` or in the nearest directory above it.
    #[must_use]
    pub fn find(start: &Path) -> Option<PathBuf> {
        start
            .ancestors()
            .map(|dir| dir.join(FILE_NAME))
            .find(|path| path.is_file())
    }

    /// This configuration with every setting `over` sets taken from `over`.
    #[must_use]
    pub fn overridden_by(self, over: Self) -> Self {
        Self {
            max_width: over.max_width.or(self.max_width),
            format_markdown: over.format_markdown.or(self.format_markdown),
            reference_links: over.reference_links.or(self.reference_links),
            prune_reference_links: over.prune_reference_links.or(self.prune_reference_links),
        }
    }

    /// The options to format with: each setting as configured, or its default.
    #[must_use]
    pub fn format_options(&self) -> FormatOptions {
        FormatOptions {
            max_width: self.max_width.unwrap_or(DEFAULT_MAX_WIDTH),
            canonical: self.format_markdown.unwrap_or_default(),
            reference_links: self.reference_links.unwrap_or_default(),
            prune_reference_links: self.prune_reference_links.unwrap_or_default(),
        }
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
