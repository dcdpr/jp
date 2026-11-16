//! Plugin settings read from `plugins.command.rfd.options`.
//!
//! ```toml
//! [plugins.command.rfd.options]
//! dir = "docs/rfd"
//! categories = ["design", "decision", "guide", "process"]
//! # Author recorded on drafts created via a tool.
//! assistant = "jp"
//!
//! # Label key linking a tracking ticket to its RFD, and extra labels added
//! # when filing a tracking ticket.
//! [plugins.command.rfd.options.tracking]
//! key = "tracking-rfd"
//! labels = ["type=tracking"]
//!
//! # Label key linking a ticket to a related RFD.
//! [plugins.command.rfd.options.relevant]
//! key = "relevant-rfd"
//! ```
//!
//! `format_on_write = true` formats written RFDs using the workspace's
//! `.comfort.toml` (disabled by default).
//!
//! All settings are optional; `assistant` defaults to `"assistant"`, and
//! `tracking.labels` defaults to `[]`.
//! Unknown keys are ignored so configurations targeting a newer `jp-rfd` still
//! load.
//!
//! [`ticket_dir`] and [`ticket_vocabulary`] read the ticket directory and label
//! vocabulary from `plugins.command.ticket.options`, and [`wiki_dir`] reads the
//! wiki directory from `plugins.command.wiki.options`.

use camino::Utf8PathBuf;
use jp_label::validate_key;
use serde::Deserialize;
use serde_json::{Map, Value};
use ticket::{Vocabulary, store};

use crate::links::{DEFAULT_RELEVANT_KEY, DEFAULT_TRACKING_KEY, Links};

/// Configuration key where `jp ticket` reads the label vocabulary.
pub(crate) const TICKET_LABELS_KEY: &str = "plugins.command.ticket.options.labels";

const DEFAULT_DIR: &str = "docs/rfd";
const DEFAULT_CATEGORIES: [&str; 4] = ["design", "decision", "guide", "process"];
const DEFAULT_ASSISTANT: &str = "assistant";

/// Validated settings for `jp-rfd` commands and tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Options {
    /// RFD directory relative to the workspace root.
    /// Drafts live in `drafts/` under this directory.
    pub dir: Utf8PathBuf,

    /// Allowed RFD categories, in lowercase.
    pub categories: Vec<String>,

    /// Author recorded on drafts created via a tool.
    pub assistant: String,

    /// Label keys used to link tickets to RFDs.
    pub links: Links,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            dir: Utf8PathBuf::from(DEFAULT_DIR),
            categories: DEFAULT_CATEGORIES.map(ToOwned::to_owned).to_vec(),
            assistant: DEFAULT_ASSISTANT.to_owned(),
            links: Links::default(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct Raw {
    dir: Utf8PathBuf,
    categories: Vec<String>,
    assistant: String,
    tracking: RawTracking,
    relevant: RawRelevant,
}

impl Default for Raw {
    fn default() -> Self {
        let options = Options::default();

        Self {
            dir: options.dir,
            categories: options.categories,
            assistant: options.assistant,
            tracking: RawTracking::default(),
            relevant: RawRelevant::default(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct RawTracking {
    key: String,
    labels: Vec<String>,
}

impl Default for RawTracking {
    fn default() -> Self {
        Self {
            key: DEFAULT_TRACKING_KEY.to_owned(),
            labels: vec![],
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
    /// Parse and validate the plugin options from the `init` message.
    ///
    /// Categories are lowercased and deduplicated.
    pub(crate) fn read(options: &Map<String, Value>) -> Result<Self, String> {
        let read: Raw = serde_json::from_value(Value::Object(options.clone()))
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

        let assistant = read.assistant.trim();
        if assistant.is_empty() || assistant.chars().any(char::is_control) {
            return Err(
                "`plugins.command.rfd.options.assistant` must be a name on one line.".to_owned(),
            );
        }

        Ok(Self {
            dir: read.dir,
            categories,
            assistant: assistant.to_owned(),
            links: links(read.tracking, read.relevant)?,
        })
    }
}

/// Validate and build the ticket-to-RFD link configuration.
///
/// `tracking.key` and `relevant.key` must be distinct, and `tracking.labels`
/// cannot use either key because the RFD link label is added automatically.
fn links(tracking: RawTracking, relevant: RawRelevant) -> Result<Links, String> {
    for (setting, key) in [
        ("tracking.key", &tracking.key),
        ("relevant.key", &relevant.key),
    ] {
        validate_key(key).map_err(|error| {
            format!("`plugins.command.rfd.options.{setting}` is invalid: {error}.")
        })?;
    }

    if tracking.key.eq_ignore_ascii_case(&relevant.key) {
        return Err(format!(
            "`plugins.command.rfd.options.tracking.key` and `relevant.key` are both `{}`; a \
             ticket naming an RFD under it would both track it and only relate to it.",
            tracking.key
        ));
    }

    for label in &tracking.labels {
        let key = label.split_once('=').map_or(label.as_str(), |(key, _)| key);
        if key.eq_ignore_ascii_case(&tracking.key) || key.eq_ignore_ascii_case(&relevant.key) {
            return Err(format!(
                "`plugins.command.rfd.options.tracking.labels` holds `{label}`; the label naming \
                 the RFD is added on its own."
            ));
        }
    }

    Ok(Links {
        tracking: tracking.key,
        relevant: relevant.key,
        filed_with: tracking.labels,
    })
}

/// Read the ticket directory from `plugins.command.ticket.options.dir` in the
/// host configuration, defaulting to `docs/ticket`.
pub(crate) fn ticket_dir(config: &Value) -> Result<Utf8PathBuf, String> {
    match config.pointer("/plugins/command/ticket/options/dir") {
        None | Some(Value::Null) => Ok(Utf8PathBuf::from(store::DEFAULT_DIR)),
        Some(Value::String(dir)) if !dir.trim().is_empty() => Ok(Utf8PathBuf::from(dir)),
        Some(_) => Err("`plugins.command.ticket.options.dir` is not a directory path.".to_owned()),
    }
}

const DEFAULT_WIKI_DIR: &str = "docs/wiki";

/// Read the wiki directory from `plugins.command.wiki.options.dir` in the host
/// configuration, defaulting to `docs/wiki`.
pub(crate) fn wiki_dir(config: &Value) -> Result<Utf8PathBuf, String> {
    match config.pointer("/plugins/command/wiki/options/dir") {
        None | Some(Value::Null) => Ok(Utf8PathBuf::from(DEFAULT_WIKI_DIR)),
        Some(Value::String(dir)) if !dir.trim().is_empty() => Ok(Utf8PathBuf::from(dir)),
        Some(_) => Err("`plugins.command.wiki.options.dir` is not a directory path.".to_owned()),
    }
}

/// Read the ticket label vocabulary from
/// `plugins.command.ticket.options.labels` in the host configuration,
/// defaulting to an empty vocabulary.
pub(crate) fn ticket_vocabulary(config: &Value) -> Result<Vocabulary, String> {
    match config.pointer("/plugins/command/ticket/options/labels") {
        None | Some(Value::Null) => Ok(Vocabulary::default()),
        Some(labels) => Vocabulary::from_value(labels.clone())
            .map_err(|error| format!("`{TICKET_LABELS_KEY}`: {error}")),
    }
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
