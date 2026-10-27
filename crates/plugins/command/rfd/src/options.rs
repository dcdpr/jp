//! The plugin's settings, read from `plugins.command.rfd.options`.
//!
//! ```toml
//! [plugins.command.rfd.options]
//! dir = "docs/rfd"
//! categories = ["design", "decision", "guide", "process"]
//!
//! # The label a ticket names the RFD it tracks under, and the labels a
//! # tracking ticket is filed with besides it.
//! [plugins.command.rfd.options.tracking]
//! key = "tracking-rfd"
//! labels = ["type=tracking"]
//!
//! # The label a ticket names an RFD its work belongs to under.
//! [plugins.command.rfd.options.relevant]
//! key = "relevant-rfd"
//! ```
//!
//! Every option is optional, and `tracking.labels` defaults to none.
//! Keys this plugin doesn't read are ignored, so a configuration written for a
//! newer `jp-rfd` still loads.
//!
//! The ticket directory belongs to `jp ticket`, and is read from its options
//! ([`ticket_dir`]), as are the labels a ticket can carry
//! ([`ticket_vocabulary`]).

use camino::Utf8PathBuf;
use jp_label::validate_key;
use serde::Deserialize;
use serde_json::{Map, Value};
use ticket::{Vocabulary, store};

use crate::links::{DEFAULT_RELEVANT_KEY, DEFAULT_TRACKING_KEY, Links};

/// Where `jp ticket` configures the label vocabulary, as a user writes it.
pub(crate) const TICKET_LABELS_KEY: &str = "plugins.command.ticket.options.labels";

/// Where the RFDs live when `dir` isn't set.
const DEFAULT_DIR: &str = "docs/rfd";

/// The categories when `categories` isn't set.
const DEFAULT_CATEGORIES: [&str; 4] = ["design", "decision", "guide", "process"];

/// The settings every command and tool runs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Options {
    /// Directory holding the RFDs, relative to the workspace root.
    /// Drafts live in its `drafts/` directory.
    pub dir: Utf8PathBuf,

    /// The categories an RFD can carry, lowercase.
    pub categories: Vec<String>,

    /// The label keys tickets name RFDs under.
    pub links: Links,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            dir: Utf8PathBuf::from(DEFAULT_DIR),
            categories: DEFAULT_CATEGORIES.map(ToOwned::to_owned).to_vec(),
            links: Links::default(),
        }
    }
}

/// The options as the configuration writes them.
#[derive(Debug, Deserialize)]
#[serde(default)]
struct Raw {
    dir: Utf8PathBuf,
    categories: Vec<String>,
    tracking: RawTracking,
    relevant: RawRelevant,
}

impl Default for Raw {
    fn default() -> Self {
        let options = Options::default();

        Self {
            dir: options.dir,
            categories: options.categories,
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
    /// Read the options the host passed in `init`.
    ///
    /// Categories are lowercased and deduplicated.
    /// The error names the setting and says what is wrong with it, for the user
    /// to fix in their configuration.
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

        Ok(Self {
            dir: read.dir,
            categories,
            links: links(read.tracking, read.relevant)?,
        })
    }
}

/// The link keys and the labels a tracking ticket is filed with, checked.
///
/// The two keys have to differ, or every ticket naming an RFD would track it,
/// and a filed-with label can't sit under either, since the link is added on
/// its own.
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

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
