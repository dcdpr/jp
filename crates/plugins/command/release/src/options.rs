//! The plugin's settings, read from `plugins.command.release.options`.
//!
//! ```toml
//! [plugins.command.release.options]
//! fragments_dir = "changelog.d"
//! ```
//!
//! `format_on_write = true` lays every fragment, highlight, and README the
//! plugin writes out in the style of the workspace's `.comfort.toml`; it is off
//! by default.
//!
//! Every option is optional.
//! Keys this plugin doesn't read are ignored, so a configuration written for a
//! newer `jp-release` still loads.

use camino::Utf8PathBuf;
use serde::Deserialize;
use serde_json::{Map, Value};

/// The fragments directory, when `fragments_dir` isn't set.
const DEFAULT_FRAGMENTS_DIR: &str = "changelog.d";

/// The settings every command and tool runs with.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct Options {
    /// The directory holding the fragments and highlights, relative to the
    /// workspace root.
    pub fragments_dir: Utf8PathBuf,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            fragments_dir: Utf8PathBuf::from(DEFAULT_FRAGMENTS_DIR),
        }
    }
}

impl Options {
    /// Read the options the host passed in `init`.
    ///
    /// The error names the setting and says what is wrong with it, for the user
    /// to fix in their configuration.
    pub(crate) fn read(options: &Map<String, Value>) -> Result<Self, String> {
        let read: Self = serde_json::from_value(Value::Object(options.clone()))
            .map_err(|error| format!("`plugins.command.release.options` is invalid: {error}."))?;

        if read.fragments_dir.as_str().trim().is_empty() {
            return Err("`plugins.command.release.options.fragments_dir` is empty.".to_owned());
        }

        Ok(read)
    }
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
