//! Configuration under `plugins.command.release-github.options`.
//!
//! ```toml
//! [plugins.command.release-github.options]
//! assets = [".config/release/install.sh", ".config/release/install.ps1"]
//! upgrade_guide = "https://jp.computer/releases/{version}/upgrade"
//! footer = ".config/release/github.md"
//! ```
//!
//! Shared release-note settings (`fragments_dir`, `forge`, `repository`,
//! `headings`) are loaded from `plugins.command.release.options` via
//! [`release_options::Notes`].

use camino::Utf8PathBuf;
use doc::repository::is_web_address;
use serde::Deserialize;
use serde_json::{Map, Value};

/// Version placeholder in `upgrade_guide` and `footer` templates.
pub(crate) const VERSION: &str = "{version}";

/// Download base-URL placeholder in attached `assets` and `footer` templates.
pub(crate) const DOWNLOAD: &str = "{download}";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Options {
    /// Extra workspace-relative files to attach to every GitHub Release (with
    /// `{download}` substituted in UTF-8 files).
    pub(crate) assets: Vec<Utf8PathBuf>,

    /// External upgrade guide URL template containing `{version}`; if `None`,
    /// the upgrade guide is appended inline to the release description.
    pub(crate) upgrade_guide: Option<String>,

    /// Workspace-relative Markdown file appended to every GitHub Release
    /// description (with `{version}` and `{download}` substituted).
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
    /// Parse [`Options`] from `plugins.command.release-github.options`,
    /// ignoring unknown keys.
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
