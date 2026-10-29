//! The plugin's settings, read from `plugins.command.ticket.options`.
//!
//! ```toml
//! [plugins.command.ticket.options]
//! dir = "docs/ticket"
//! assistant = "jp"
//! board.done_limit = 8
//!
//! [plugins.command.ticket.options.labels.type]
//! description = "The type of ticket"
//! values = ["bug", "feature", "tracking"]
//! ```
//!
//! Every option is optional.
//! Keys this plugin doesn't read are ignored, so a configuration written for a
//! newer `jp-ticket` still loads.

use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;
use serde_json::{Map, Value};
use ticket::{Vocabulary, store};

/// Where the label vocabulary is configured, as a user writes it.
pub(crate) const LABELS_KEY: &str = "plugins.command.ticket.options.labels";

/// The file the vocabulary was read from before it moved into `labels`.
const LEGACY_LABELS_FILE: &str = ".labels.json";

/// The author written on tickets and comments filed through a tool, when
/// `assistant` isn't set.
const DEFAULT_ASSISTANT: &str = "assistant";

/// How many Done tickets the board shows, when `board.done_limit` isn't set.
const DEFAULT_DONE_LIMIT: usize = 8;

/// The settings every command and tool runs with.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct Options {
    /// Directory holding the ticket files, relative to the workspace root.
    pub dir: Utf8PathBuf,

    /// The author written on tickets and comments filed through a tool.
    pub assistant: String,

    /// How `jp ticket board` lays out the board.
    pub board: BoardOptions,

    /// The label vocabulary, as configured: a table per key, holding its
    /// `description`, its `values`, and the values `retired` from it.
    ///
    /// Kept as written and read by [`Options::vocabulary`], so a vocabulary
    /// that doesn't parse fails only the commands that label.
    pub labels: Option<Value>,
}

/// How `jp ticket board` lays out the board.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct BoardOptions {
    /// How many Done tickets the board shows, from the top of the column.
    pub done_limit: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            dir: Utf8PathBuf::from(store::DEFAULT_DIR),
            assistant: DEFAULT_ASSISTANT.to_owned(),
            board: BoardOptions::default(),
            labels: None,
        }
    }
}

impl Default for BoardOptions {
    fn default() -> Self {
        Self {
            done_limit: DEFAULT_DONE_LIMIT,
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
            .map_err(|error| format!("`plugins.command.ticket.options` is invalid: {error}."))?;

        if read.dir.as_str().trim().is_empty() {
            return Err("`plugins.command.ticket.options.dir` is empty.".to_owned());
        }

        let assistant = read.assistant.trim();
        if assistant.is_empty() || assistant.chars().any(char::is_control) {
            return Err(
                "`plugins.command.ticket.options.assistant` must be a name on one line.".to_owned(),
            );
        }

        Ok(Self {
            assistant: assistant.to_owned(),
            ..read
        })
    }

    /// The labels the board defines.
    ///
    /// A board that configures none defines none, which is a board that hasn't
    /// started using them rather than an error.
    pub(crate) fn vocabulary(&self) -> Result<Vocabulary, String> {
        match &self.labels {
            None => Ok(Vocabulary::default()),
            Some(labels) => Vocabulary::from_value(labels.clone())
                .map_err(|error| format!("`{LABELS_KEY}`: {error}")),
        }
    }
}

/// The file in `dir` a board defined its labels in before they moved into
/// `labels`, when one is still there.
///
/// Nothing reads it, so a board that kept it defines no labels until its keys
/// are moved.
pub(crate) fn leftover_labels_file(dir: &Utf8Path) -> Option<Utf8PathBuf> {
    let path = dir.join(LEGACY_LABELS_FILE);

    path.is_file().then_some(path)
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
