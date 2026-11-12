//! `jp release`'s settings that other plugins read too: where the fragments
//! are, and how release notes credit and head their entries.
//!
//! They live in `plugins.command.release.options`, beside settings only `jp
//! release` reads, which this crate ignores.
//! [`Notes::read`] reads them from the options the host passes `jp release`,
//! and [`Notes::from_config`] from the whole configuration another plugin is
//! passed.
//! Both hold them to the same rules, so every plugin refuses the same values in
//! the same words.

use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use changelog::{Forge, Headings, Type, UnknownType};
use doc::repository::is_web_address;
use serde::Deserialize;
use serde_json::{Map, Value};

/// The fragments directory, when `fragments_dir` isn't set.
pub const DEFAULT_FRAGMENTS_DIR: &str = "changelog.d";

/// Where the settings are, as errors name them.
const KEY: &str = "plugins.command.release.options";

/// The setting `repository` is read from, as errors name it.
pub const REPOSITORY_KEY: &str = "plugins.command.release.options.repository";

/// The settings release notes are found and rendered with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notes {
    /// The directory holding the fragments and highlights, relative to the
    /// workspace root unless absolute.
    pub fragments_dir: Utf8PathBuf,

    /// The forge release notes credit change requests on; `None` reads it from
    /// the project's web address.
    pub forge: Option<Forge>,

    /// The project's web address, which credits link from, without a `/` at its
    /// end; `None` takes git's `origin` remote's.
    pub repository: Option<String>,

    /// The heading of each type's section of the release notes.
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

/// The settings as the configuration writes them.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Section {
    fragments_dir: Option<Utf8PathBuf>,
    forge: Option<String>,
    repository: Option<String>,
    headings: BTreeMap<String, String>,
}

impl Notes {
    /// Read the settings from `options`, the options the host passes `jp
    /// release`.
    ///
    /// # Errors
    ///
    /// Names the setting and says what is wrong with it, for the user to fix in
    /// their configuration.
    pub fn read(options: &Map<String, Value>) -> Result<Self, String> {
        Self::parse(Value::Object(options.clone()))
    }

    /// Read the settings from `config`, the whole configuration the host passes
    /// a plugin.
    ///
    /// # Errors
    ///
    /// As [`Notes::read`].
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

/// Why release notes can't credit change requests on `forge`: there is no web
/// address for the credits to link to, and how to give one.
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
