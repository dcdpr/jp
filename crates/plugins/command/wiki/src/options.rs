//! The plugin's settings, read from `plugins.command.wiki.options`, and the
//! directories of the documents a wiki page links to.
//!
//! ```toml
//! [plugins.command.wiki.options]
//! dir = "docs/wiki"
//! assistant = "jp"
//! # The label key a wiki page uses to link to an RFD.
//! relevant.key = "relevant-rfd"
//!
//! [plugins.command.wiki.options.labels.domain]
//! description = "The subsystem covered by this page"
//! values = ["conversation", "llm"]
//! ```
//!
//! Every option is optional.
//! Keys this plugin doesn't read are ignored, so a configuration written for a
//! newer `jp-wiki` still loads.
//!
//! The RFD and ticket directories belong to `jp rfd` and `jp ticket`, and the
//! repository address to `jp vite`; [`Paths::new`] and [`repository`] read them
//! from the whole configuration.

use std::path::MAIN_SEPARATOR;

use camino::{Utf8Path, Utf8PathBuf};
use doc::repository::{is_web_address, web_address};
use jp_label::{Vocabulary, validate_key};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::git;

/// Where the label vocabulary is configured, as a user writes it.
pub(crate) const LABELS_KEY: &str = "plugins.command.wiki.options.labels";

/// Where the pages live when `dir` isn't set.
const DEFAULT_DIR: &str = "docs/wiki";

/// The author written on pages created through a tool, when `assistant` isn't
/// set.
const DEFAULT_ASSISTANT: &str = "assistant";

/// The key a page names an RFD it relates to under when `relevant.key` isn't
/// set.
const DEFAULT_RELEVANT_KEY: &str = "relevant-rfd";

/// Where `jp rfd` keeps the RFDs when its `dir` isn't set.
const DEFAULT_RFD_DIR: &str = "docs/rfd";

/// The settings every command runs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Options {
    /// Directory holding the pages, relative to the workspace root.
    pub dir: Utf8PathBuf,

    /// The author written on pages created through a tool.
    pub assistant: String,

    /// The label key a page names an RFD it relates to under.
    pub relevant_key: String,

    /// The label vocabulary, as configured, read by [`Options::vocabulary`] so
    /// a vocabulary that doesn't parse fails only the commands that label.
    pub labels: Option<Value>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            dir: Utf8PathBuf::from(DEFAULT_DIR),
            assistant: DEFAULT_ASSISTANT.to_owned(),
            relevant_key: DEFAULT_RELEVANT_KEY.to_owned(),
            labels: None,
        }
    }
}

/// The options as the configuration writes them.
#[derive(Debug, Deserialize)]
#[serde(default)]
struct Raw {
    dir: Utf8PathBuf,
    assistant: String,
    relevant: RawRelevant,
    labels: Option<Value>,
}

impl Default for Raw {
    fn default() -> Self {
        let options = Options::default();

        Self {
            dir: options.dir,
            assistant: options.assistant,
            relevant: RawRelevant::default(),
            labels: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct RawRelevant {
    key: String,
}

impl Default for RawRelevant {
    fn default() -> Self {
        Self {
            key: DEFAULT_RELEVANT_KEY.to_owned(),
        }
    }
}

impl Options {
    /// Read the options the host passed in `init`.
    ///
    /// The error names the setting and says what is wrong with it, for the user
    /// to fix in their configuration.
    pub(crate) fn read(options: &Map<String, Value>) -> Result<Self, String> {
        let read: Raw = serde_json::from_value(Value::Object(options.clone()))
            .map_err(|error| format!("`plugins.command.wiki.options` is invalid: {error}."))?;

        if read.dir.as_str().trim().is_empty() {
            return Err("`plugins.command.wiki.options.dir` is empty.".to_owned());
        }

        let assistant = read.assistant.trim();
        if assistant.is_empty() || assistant.chars().any(char::is_control) {
            return Err(
                "`plugins.command.wiki.options.assistant` must be a name on one line.".to_owned(),
            );
        }

        validate_key(&read.relevant.key).map_err(|error| {
            format!("`plugins.command.wiki.options.relevant.key` is invalid: {error}.")
        })?;

        Ok(Self {
            dir: read.dir,
            assistant: assistant.to_owned(),
            relevant_key: read.relevant.key,
            labels: read.labels,
        })
    }

    /// The labels a page can carry.
    ///
    /// A wiki that configures none takes any well-formed label.
    pub(crate) fn vocabulary(&self) -> Result<Vocabulary, String> {
        match &self.labels {
            None | Some(Value::Null) => Ok(Vocabulary::default()),
            Some(labels) => Vocabulary::from_value(labels.clone())
                .map_err(|error| format!("`{LABELS_KEY}`: {error}")),
        }
    }
}

/// The directories a command reads and writes, each an absolute path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Paths {
    /// The workspace root.
    pub root: Utf8PathBuf,

    /// The wiki directory.
    pub wiki: Utf8PathBuf,

    /// The RFD directory, as `jp rfd` is configured to use it; drafts live in
    /// its `drafts/` directory.
    pub rfds: Utf8PathBuf,

    /// The ticket directory, as `jp ticket` is configured to use it.
    pub tickets: Utf8PathBuf,
}

impl Paths {
    /// The directories for the workspace at `root`, as `config`, the whole
    /// configuration the host passed, and the wiki's `options` set them.
    pub(crate) fn new(root: &Utf8Path, config: &Value, options: &Options) -> Result<Self, String> {
        let resolve = |dir: &Utf8Path| {
            if dir.is_absolute() {
                dir.to_path_buf()
            } else {
                root.join(dir)
            }
        };

        Ok(Self {
            root: root.to_path_buf(),
            wiki: resolve(&options.dir),
            rfds: resolve(&directory(config, "rfd", DEFAULT_RFD_DIR)?),
            tickets: resolve(&directory(config, "ticket", ticket::store::DEFAULT_DIR)?),
        })
    }

    /// The documentation tree the wiki directory sits in, which a page's
    /// relative links stay inside unless they cite source: `docs` for
    /// `docs/wiki`.
    ///
    /// The wiki directory itself when its parent is the workspace root or lies
    /// outside it.
    #[must_use]
    pub(crate) fn docs(&self) -> Utf8PathBuf {
        self.wiki
            .parent()
            .filter(|parent| *parent != self.root && parent.starts_with(&self.root))
            .map_or_else(|| self.wiki.clone(), Utf8Path::to_path_buf)
    }

    /// `path` the way the user reads it: relative to the workspace root, with
    /// `/` separators.
    #[must_use]
    pub(crate) fn show(&self, path: &Utf8Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .as_str()
            .replace(MAIN_SEPARATOR, "/")
    }
}

/// A plugin's configured directory, or its default.
fn directory(config: &Value, plugin: &str, default: &str) -> Result<Utf8PathBuf, String> {
    match config.pointer(&format!("/plugins/command/{plugin}/options/dir")) {
        None | Some(Value::Null) => Ok(Utf8PathBuf::from(default)),
        Some(Value::String(dir)) if !dir.trim().is_empty() => Ok(Utf8PathBuf::from(dir)),
        Some(_) => Err(format!(
            "`plugins.command.{plugin}.options.dir` is not a directory path."
        )),
    }
}

/// The web address a permalink names the repository by: `jp vite`'s
/// `plugins.command.vite.options.repository`, or the address of git's `origin`
/// remote.
///
/// `None` when neither gives one.
pub(crate) fn repository(root: &Utf8Path, config: &Value) -> Result<Option<String>, String> {
    match config.pointer("/plugins/command/vite/options/repository") {
        None | Some(Value::Null) => Ok(git::origin(root).as_deref().and_then(web_address)),
        Some(Value::String(address)) if is_web_address(address) => {
            Ok(Some(address.trim_end_matches('/').to_owned()))
        }
        Some(_) => Err(
            "`plugins.command.vite.options.repository` must be an `https://` address.".to_owned(),
        ),
    }
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
