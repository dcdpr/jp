//! The plugin's settings, read from `plugins.command.rfd.options`.
//!
//! ```toml
//! [plugins.command.rfd.options]
//! dir = "docs/rfd"
//! categories = ["design", "decision", "guide", "process"]
//! ```
//!
//! Every option is optional.
//! Keys this plugin doesn't read are ignored, so a configuration written for a
//! newer `jp-rfd` still loads.
//!
//! The ticket directory belongs to `jp ticket`, and is read from its options
//! ([`ticket_dir`]), as are who a ticket filed through a tool is attributed to
//! ([`assistant`]) and the labels a ticket can carry ([`ticket_vocabulary`]).

use camino::Utf8PathBuf;
use serde::Deserialize;
use serde_json::{Map, Value};
use ticket::{Vocabulary, store};

/// Where `jp ticket` configures the label vocabulary, as a user writes it.
pub(crate) const TICKET_LABELS_KEY: &str = "plugins.command.ticket.options.labels";

/// Where the RFDs live when `dir` isn't set.
const DEFAULT_DIR: &str = "docs/rfd";

/// The categories when `categories` isn't set.
const DEFAULT_CATEGORIES: [&str; 4] = ["design", "decision", "guide", "process"];

/// Who a ticket filed through a tool is attributed to, when `jp ticket` sets
/// nobody.
const DEFAULT_ASSISTANT: &str = "assistant";

/// The settings every command and tool runs with.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct Options {
    /// Directory holding the RFDs, relative to the workspace root.
    /// Drafts live in its `drafts/` directory.
    pub dir: Utf8PathBuf,

    /// The categories an RFD can carry, lowercase.
    pub categories: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            dir: Utf8PathBuf::from(DEFAULT_DIR),
            categories: DEFAULT_CATEGORIES.map(ToOwned::to_owned).to_vec(),
        }
    }
}

impl Options {
    /// Read the options the host passed in `init`.
    ///
    /// Categories are lowercased and deduplicated.
    /// The error names the setting and says what is wrong with it, for the user
    /// to fix in their configuration.
    pub(crate) fn read(options: &Map<String, Value>) -> Result<Self, String> {
        let read: Self = serde_json::from_value(Value::Object(options.clone()))
            .map_err(|error| format!("`plugins.command.rfd.options` is invalid: {error}."))?;

        if read.dir.as_str().trim().is_empty() {
            return Err("`plugins.command.rfd.options.dir` is empty.".to_owned());
        }

        let mut categories: Vec<String> = vec![];
        for category in &read.categories {
            let category = category.trim().to_lowercase();
            let usable = category
                .chars()
                .next()
                .is_some_and(|first| first.is_ascii_alphanumeric())
                && category
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-');
            if !usable {
                return Err(format!(
                    "`plugins.command.rfd.options.categories` holds `{category}`; a category is \
                     letters, digits, and dashes, like `design`."
                ));
            }
            if !categories.contains(&category) {
                categories.push(category);
            }
        }

        if categories.is_empty() {
            return Err(
                "`plugins.command.rfd.options.categories` is empty; name at least one category."
                    .to_owned(),
            );
        }

        Ok(Self {
            dir: read.dir,
            categories,
        })
    }
}

/// The ticket directory, as `jp ticket` is configured to use it: its
/// `plugins.command.ticket.options.dir`, or `docs/ticket`.
///
/// `config` is the whole configuration the host passed in `init`.
pub(crate) fn ticket_dir(config: &Value) -> Result<Utf8PathBuf, String> {
    match config.pointer("/plugins/command/ticket/options/dir") {
        None | Some(Value::Null) => Ok(Utf8PathBuf::from(store::DEFAULT_DIR)),
        Some(Value::String(dir)) if !dir.trim().is_empty() => Ok(Utf8PathBuf::from(dir)),
        Some(_) => Err("`plugins.command.ticket.options.dir` is not a directory path.".to_owned()),
    }
}

/// The labels a ticket can carry, as `jp ticket` is configured: its
/// `plugins.command.ticket.options.labels`, or none.
///
/// `config` is the whole configuration the host passed in `init`.
pub(crate) fn ticket_vocabulary(config: &Value) -> Result<Vocabulary, String> {
    match config.pointer("/plugins/command/ticket/options/labels") {
        None | Some(Value::Null) => Ok(Vocabulary::default()),
        Some(labels) => Vocabulary::from_value(labels.clone())
            .map_err(|error| format!("`{TICKET_LABELS_KEY}`: {error}")),
    }
}

/// Who a ticket filed through a tool is attributed to, as `jp ticket` is
/// configured: its `plugins.command.ticket.options.assistant`, or `assistant`.
///
/// `config` is the whole configuration the host passed in `init`.
pub(crate) fn assistant(config: &Value) -> Result<String, String> {
    match config.pointer("/plugins/command/ticket/options/assistant") {
        None | Some(Value::Null) => Ok(DEFAULT_ASSISTANT.to_owned()),
        Some(Value::String(name))
            if !name.trim().is_empty() && !name.chars().any(char::is_control) =>
        {
            Ok(name.trim().to_owned())
        }
        Some(_) => {
            Err("`plugins.command.ticket.options.assistant` must be a name on one line.".to_owned())
        }
    }
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
