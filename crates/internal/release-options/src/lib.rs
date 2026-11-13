//! Shared `plugins.command.release.options` settings consumed across release
//! and site plugins.
//!
//! [`Notes::read`] parses these settings from the plugin options map passed to
//! `jp release`, and [`Notes::from_config`] extracts them from the full
//! workspace configuration passed to other plugins (such as `jp vite` and `jp
//! release github`).
//! Unknown keys in `plugins.command.release.options` are ignored so settings
//! specific to `jp release` do not fail other plugins.

use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use changelog::{Forge, Headings, Type, UnknownType};
use doc::repository::is_web_address;
use serde::Deserialize;
use serde_json::{Map, Value};

/// Default fragments directory path when `fragments_dir` is unset.
pub const DEFAULT_FRAGMENTS_DIR: &str = "changelog.d";

const KEY: &str = "plugins.command.release.options";

/// Configuration key path for the `repository` option, used in error messages.
pub const REPOSITORY_KEY: &str = "plugins.command.release.options.repository";

/// Release-note configuration shared by `jp release`, `jp release github`, and
/// `jp vite`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notes {
    /// Path to the fragments directory, relative to the workspace root unless
    /// absolute.
    pub fragments_dir: Utf8PathBuf,

    /// Forge where change requests are reviewed; `None` infers the forge from
    /// the repository URL.
    pub forge: Option<Forge>,

    /// Base web URL of the repository (without trailing `/`), or `None` to
    /// derive it from git's `origin` remote.
    pub repository: Option<String>,

    /// Custom section headings for changelog fragment types.
    pub headings: Headings,
}

impl Default for Notes {
    fn default() -> Self {
        Self {
            fragments_dir: Utf8PathBuf::from(DEFAULT_FRAGMENTS_DIR),
            forge: None,
            repository: None,
            headings: Headings::default(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Section {
    fragments_dir: Option<Utf8PathBuf>,
    forge: Option<String>,
    repository: Option<String>,
    headings: BTreeMap<String, String>,
}

impl Notes {
    /// Parse [`Notes`] from the `plugins.command.release.options` map.
    ///
    /// # Errors
    ///
    /// Returns an error naming the invalid configuration key.
    pub fn read(options: &Map<String, Value>) -> Result<Self, String> {
        Self::parse(Value::Object(options.clone()))
    }

    /// Extract and parse [`Notes`] from a full workspace configuration object.
    ///
    /// # Errors
    ///
    /// Returns an error naming the invalid configuration key.
    pub fn from_config(config: &Value) -> Result<Self, String> {
        match config.pointer("/plugins/command/release/options") {
            None | Some(Value::Null) => Ok(Self::default()),
            Some(options) => Self::parse(options.clone()),
        }
    }

    fn parse(options: Value) -> Result<Self, String> {
        let section: Section = serde_json::from_value(options)
            .map_err(|error| format!("`{KEY}` is invalid: {error}."))?;

        Ok(Self {
            fragments_dir: fragments_dir(section.fragments_dir)?,
            forge: forge(section.forge)?,
            repository: repository(section.repository)?,
            headings: headings(&section.headings)?,
        })
    }
}

/// Format the error returned when `forge` requires a repository web URL for
/// credit links and none is available from git's `origin` remote or
/// [`REPOSITORY_KEY`].
#[must_use]
pub fn no_address(forge: Forge) -> String {
    format!(
        "The forge is `{forge}`, but git's `origin` remote has no web address for credits to link \
         to; set `{REPOSITORY_KEY}` to the project's, such as `https://github.com/dcdpr/jp`."
    )
}

fn fragments_dir(configured: Option<Utf8PathBuf>) -> Result<Utf8PathBuf, String> {
    match configured {
        None => Ok(Utf8PathBuf::from(DEFAULT_FRAGMENTS_DIR)),
        Some(dir) if dir.as_str().trim().is_empty() => {
            Err(format!("`{KEY}.fragments_dir` is empty."))
        }
        Some(dir) => Ok(dir),
    }
}

fn forge(configured: Option<String>) -> Result<Option<Forge>, String> {
    configured
        .map(|forge| forge.parse::<Forge>())
        .transpose()
        .map_err(|error| format!("`{KEY}.forge`: {error}."))
}

fn repository(configured: Option<String>) -> Result<Option<String>, String> {
    match configured {
        None => Ok(None),
        Some(repository) if is_web_address(&repository) => {
            Ok(Some(repository.trim_end_matches('/').to_owned()))
        }
        Some(_) => Err(format!("`{REPOSITORY_KEY}` must be an `https://` address.")),
    }
}

fn headings(configured: &BTreeMap<String, String>) -> Result<Headings, String> {
    let mut headings = Headings::default();
    for (type_, heading) in configured {
        let type_ = type_
            .parse::<Type>()
            .map_err(|error: UnknownType| format!("`{KEY}.headings`: {error}."))?;
        let heading = heading.trim();
        if heading.is_empty() {
            return Err(format!("`{KEY}.headings.{type_}` is empty."));
        }
        headings.set(type_, heading);
    }

    Ok(headings)
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
