//! The plugin's settings, read from `plugins.command.release.options`.
//!
//! ```toml
//! [plugins.command.release.options]
//! fragments_dir = "changelog.d"
//! forge = "github"
//!
//! [plugins.command.release.options.headings]
//! added = "New features"
//! fixed = "Bug fixes"
//! ```
//!
//! `format_on_write = true` lays every fragment, highlight, and README the
//! plugin writes out in the style of the workspace's `.comfort.toml`; it is off
//! by default.
//!
//! `forge` is where change requests are reviewed, `github`, `gitlab`, or
//! `none`, which decides how release notes credit them and where the credits
//! link.
//! Unset, it is read from git's `origin` remote: `github` for github.com,
//! `gitlab` for gitlab.com, and `none` for any other host.
//! `headings` renames a type's section of the release notes; a type it doesn't
//! name is headed by its id capitalized, such as `Added`.
//!
//! Every option is optional.
//! Keys this plugin doesn't read are ignored, so a configuration written for a
//! newer `jp-release` still loads.

use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use changelog::{Forge, Headings, Type, UnknownType};
use serde::Deserialize;
use serde_json::{Map, Value};

/// The fragments directory, when `fragments_dir` isn't set.
const DEFAULT_FRAGMENTS_DIR: &str = "changelog.d";

/// The settings every command and tool runs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Options {
    /// The directory holding the fragments and highlights, relative to the
    /// workspace root.
    pub fragments_dir: Utf8PathBuf,

    /// The forge release notes credit change requests on; `None` reads it from
    /// git's `origin` remote.
    pub forge: Option<Forge>,

    /// The heading of each type's section of the release notes.
    pub headings: Headings,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            fragments_dir: Utf8PathBuf::from(DEFAULT_FRAGMENTS_DIR),
            forge: None,
            headings: Headings::default(),
        }
    }
}

/// The options as the configuration writes them.
#[derive(Deserialize)]
#[serde(default)]
struct Settings {
    fragments_dir: Utf8PathBuf,
    forge: Option<String>,
    headings: BTreeMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            fragments_dir: Utf8PathBuf::from(DEFAULT_FRAGMENTS_DIR),
            forge: None,
            headings: BTreeMap::new(),
        }
    }
}

impl Options {
    /// Read the options the host passed in `init`.
    ///
    /// The error names the setting and says what is wrong with it, for the user
    /// to fix in their configuration.
    pub(crate) fn read(options: &Map<String, Value>) -> Result<Self, String> {
        let settings: Settings = serde_json::from_value(Value::Object(options.clone()))
            .map_err(|error| format!("`plugins.command.release.options` is invalid: {error}."))?;

        if settings.fragments_dir.as_str().trim().is_empty() {
            return Err("`plugins.command.release.options.fragments_dir` is empty.".to_owned());
        }

        let forge = settings
            .forge
            .map(|forge| forge.parse::<Forge>())
            .transpose()
            .map_err(|error| format!("`plugins.command.release.options.forge`: {error}."))?;

        let mut headings = Headings::default();
        for (type_, heading) in &settings.headings {
            let type_ = type_.parse::<Type>().map_err(|error: UnknownType| {
                format!("`plugins.command.release.options.headings`: {error}.")
            })?;
            let heading = heading.trim();
            if heading.is_empty() {
                return Err(format!(
                    "`plugins.command.release.options.headings.{type_}` is empty."
                ));
            }
            headings.set(type_, heading);
        }

        Ok(Self {
            fragments_dir: settings.fragments_dir,
            forge,
            headings,
        })
    }
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
