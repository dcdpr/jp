//! The order a release lists its highlights and entries in.
//!
//! A release's directory may hold an `.order.json` naming its files in the
//! order the release notes list them:
//!
//! ```json
//! {
//!   "highlights": ["export-conversations.md"],
//!   "entries": ["query-command.added.md", "conversation-show.added.md"]
//! }
//! ```
//!
//! Highlights are named by their file name in `highlights/`, entries by their
//! file name, and both lists are optional.
//! Highlights come before entries whatever the file says, and entries stay
//! grouped by type: their order applies within each type's section.
//! Files a list leaves out follow the ones it names, in their default order,
//! and a name no file has is skipped, so the list stays valid as entries come
//! and go.

use std::{fmt, fs, io};

use camino::Utf8Path;
use serde::Deserialize;

/// The order file in a release's directory.
pub const ORDER: &str = ".order.json";

/// The order a release's `.order.json` gives its highlights and entries.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Order {
    /// Highlights by file name in `highlights/`, in the order they're listed.
    #[serde(default)]
    pub highlights: Vec<String>,

    /// Entries by file name, in the order they're listed within their type.
    #[serde(default)]
    pub entries: Vec<String>,
}

impl Order {
    /// Parse the text of an `.order.json`.
    ///
    /// # Errors
    ///
    /// Returns [`OrderError::Malformed`] if `text` isn't a JSON object of
    /// `highlights` and `entries`, each a list of file names.
    pub fn parse(text: &str) -> Result<Self, OrderError> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        serde_json::from_str(text).map_err(|error| {
            OrderError::Malformed(format!(
                "not an order, which is a JSON object of `highlights` and `entries`, each a list \
                 of file names: {error}"
            ))
        })
    }

    /// Read the `.order.json` of the release directory `dir`, or the default
    /// order when it has none.
    ///
    /// # Errors
    ///
    /// Returns [`OrderError::Unreadable`] if the file exists but can't be read,
    /// or [`OrderError::Malformed`] if it isn't an order.
    pub fn read(dir: &Utf8Path) -> Result<Self, OrderError> {
        match fs::read_to_string(dir.join(ORDER)) {
            Ok(text) => Self::parse(&text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(OrderError::Unreadable(error)),
        }
    }

    /// Where the highlight in `highlights/<name>` is listed, if it is.
    #[must_use]
    pub fn highlight(&self, name: &str) -> Option<usize> {
        self.highlights.iter().position(|listed| listed == name)
    }

    /// Where the entry in the file `name` is listed, if it is.
    #[must_use]
    pub fn entry(&self, name: &str) -> Option<usize> {
        self.entries.iter().position(|listed| listed == name)
    }
}

/// Error returned by [`Order::parse`] and [`Order::read`].
#[derive(Debug)]
pub enum OrderError {
    /// The file exists but can't be read.
    Unreadable(io::Error),

    /// The file isn't an order; the message says why.
    Malformed(String),
}

impl fmt::Display for OrderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(error) => write!(f, "{error}"),
            Self::Malformed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for OrderError {}

#[cfg(test)]
#[path = "order_tests.rs"]
mod tests;
