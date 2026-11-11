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
//! address, and the headings from `jp release`'s settings, held to what that
//! plugin accepts.

use camino::Utf8PathBuf;
use changelog::{Forge, Headings, Type, UnknownType};
use doc::repository::is_web_address;
use serde::Deserialize;
use serde_json::{Map, Value};

/// The fragments directory `jp release` reads when none is configured.
const DEFAULT_FRAGMENTS_DIR: &str = "changelog.d";

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

/// What the plugin reads of `jp release`'s settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Notes {
    /// The fragments directory, relative to the workspace root unless absolute.
    pub(crate) dir: Utf8PathBuf,

    /// The forge credits link to; unset, the one the web address is on.
    pub(crate) forge: Option<Forge>,

    /// The repository's web address, such as `https://github.com/dcdpr/jp`;
    /// unset, `origin`'s.
    pub(crate) repository: Option<String>,

    /// The heading each type's section has in the notes.
    pub(crate) headings: Headings,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ReleaseSection {
    fragments_dir: Option<Utf8PathBuf>,
    forge: Option<String>,
    repository: Option<String>,
    headings: Map<String, Value>,
}

impl Notes {
    /// Read `jp release`'s settings from the whole configuration the host
    /// passed.
    pub(crate) fn read(config: &Value) -> Result<Self, String> {
        let section: ReleaseSection = match config.pointer("/plugins/command/release/options") {
            None | Some(Value::Null) => ReleaseSection::default(),
            Some(options) => serde_json::from_value(options.clone()).map_err(|error| {
                format!("`plugins.command.release.options` is invalid: {error}.")
            })?,
        };

        let dir = match section.fragments_dir {
            None => Utf8PathBuf::from(DEFAULT_FRAGMENTS_DIR),
            Some(dir) if dir.as_str().trim().is_empty() => {
                return Err("`plugins.command.release.options.fragments_dir` is empty.".to_owned());
            }
            Some(dir) => dir,
        };

        let forge = section
            .forge
            .map(|forge| forge.parse::<Forge>())
            .transpose()
            .map_err(|error| format!("`plugins.command.release.options.forge`: {error}."))?;

        let repository = match section.repository {
            None => None,
            Some(repository) if is_web_address(&repository) => {
                Some(repository.trim_end_matches('/').to_owned())
            }
            Some(_) => {
                return Err(
                    "`plugins.command.release.options.repository` must be an `https://` address."
                        .to_owned(),
                );
            }
        };

        let mut headings = Headings::default();
        for (type_, heading) in &section.headings {
            let type_ = type_.parse::<Type>().map_err(|error: UnknownType| {
                format!("`plugins.command.release.options.headings`: {error}.")
            })?;
            let heading = heading.as_str().map(str::trim).unwrap_or_default();
            if heading.is_empty() {
                return Err(format!(
                    "`plugins.command.release.options.headings.{type_}` has to be a heading."
                ));
            }
            headings.set(type_, heading);
        }

        Ok(Self {
            dir,
            forge,
            repository,
            headings,
        })
    }
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
