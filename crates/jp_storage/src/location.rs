//! Storage location identifier for conversations and conversation parts.

use std::{
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
};

use camino::Utf8Path;
use serde::{Serialize, Serializer};

/// Opaque location of a stored conversation or conversation part.
///
/// The filesystem backend uses an absolute file or directory path; other
/// backends can use their own identifier format.
///
/// A location carries a full `name` and an optional short `label` (such as a
/// file name) for compact error messages.
/// Equality, hashing, ordering, and serialization compare `name` only.
#[derive(Debug, Clone)]
pub struct StorageLocation {
    name: String,
    label: Option<String>,
}

impl StorageLocation {
    /// Create a location whose label defaults to `name`.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            label: None,
        }
    }

    /// Create a location with full `name` and short display `label`.
    #[must_use]
    pub fn labelled(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            label: Some(label.into()),
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.name
    }

    /// Short display name for the location (falls back to [`Self::as_str`]).
    #[must_use]
    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }

    /// Create a location from a filesystem path, using its file name as the
    /// label.
    pub(crate) fn of_path(path: &Utf8Path) -> Self {
        match path.file_name() {
            Some(file_name) => Self::labelled(path.as_str(), file_name),
            None => Self::new(path.as_str()),
        }
    }
}

impl PartialEq for StorageLocation {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for StorageLocation {}

impl Hash for StorageLocation {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl PartialOrd for StorageLocation {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for StorageLocation {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name.cmp(&other.name)
    }
}

impl Serialize for StorageLocation {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.name)
    }
}

impl fmt::Display for StorageLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

#[cfg(test)]
#[path = "location_tests.rs"]
mod tests;
