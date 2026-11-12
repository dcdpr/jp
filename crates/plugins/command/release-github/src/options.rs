//! The plugin's settings, read from `plugins.command.release-github.options`,
//! and what it reads of `jp release`'s, from `plugins.command.release.options`.
//!
//! ```toml
//! [plugins.command.release-github.options]
//! assets = [".config/release/install.sh", ".config/release/install.ps1"]
//! upgrade_guide = "https://jp.computer/releases/{version}/upgrade"
//! footer = ".config/release/github.md"
//! ```
//!
//! A Release's description renders a release's notes the way `jp release
//! render` does, so it reads the fragments directory, the forge, the web
//! address, and the headings from `jp release`'s settings, through
//! `release_options`.

use camino::Utf8PathBuf;
use doc::repository::is_web_address;
use serde::Deserialize;
use serde_json::{Map, Value};

/// The placeholder `upgrade_guide` names a release's version with.
pub(crate) const VERSION: &str = "{version}";

/// The placeholder the files the plugin attaches name the address a release's
/// files are served from with.
pub(crate) const DOWNLOAD: &str = "{download}";

/// This plugin's settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Options {
    /// Files to attach to every Release besides its build's, relative to the
    /// workspace root, with each `{download}` in them replaced.
    pub(crate) assets: Vec<Utf8PathBuf>,

    /// The address of a release's upgrade guide, with `{version}` in it, that
    /// breaking entries link to; unset, the description includes the guide.
    pub(crate) upgrade_guide: Option<String>,

    /// A Markdown file added after the notes in every Release's description,
    /// relative to the workspace root, with `{version}` and `{download}` in it
    /// replaced.
    pub(crate) footer: Option<Utf8PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Section {
    assets: Vec<Utf8PathBuf>,
    upgrade_guide: Option<String>,
    footer: Option<Utf8PathBuf>,
}

impl Options {
    /// Read the plugin's options, as the host passed them.
    ///
    /// Settings this plugin doesn't read are ignored.
    pub(crate) fn read(options: &Map<String, Value>) -> Result<Self, String> {
        let section: Section =
            serde_json::from_value(Value::Object(options.clone())).map_err(|error| {
                format!("`plugins.command.release-github.options` is invalid: {error}.")
            })?;

        if section
            .assets
            .iter()
            .any(|asset| asset.as_str().trim().is_empty())
        {
            return Err(
                "`plugins.command.release-github.options.assets` lists an empty path.".to_owned(),
            );
        }

        let upgrade_guide = match section.upgrade_guide {
            None => None,
            Some(address) if !is_web_address(&address) => {
                return Err(
                    "`plugins.command.release-github.options.upgrade_guide` must be an `https://` \
                     address."
                        .to_owned(),
                );
            }
            Some(address) if !address.contains(VERSION) => {
                return Err(format!(
                    "`plugins.command.release-github.options.upgrade_guide` has to name the \
                     release's version as `{VERSION}`, such as \
                     `https://example.com/releases/{VERSION}/upgrade`."
                ));
            }
            Some(address) => Some(address),
        };

        let footer = match section.footer {
            Some(footer) if footer.as_str().trim().is_empty() => {
                return Err("`plugins.command.release-github.options.footer` is empty.".to_owned());
            }
            footer => footer,
        };

        Ok(Self {
            assets: section.assets,
            upgrade_guide,
            footer,
        })
    }
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
