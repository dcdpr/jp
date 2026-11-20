//! Where a storage backend holds a conversation.

use std::{
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
};

use camino::Utf8Path;
use serde::{Serialize, Serializer};

/// Where a backend holds a conversation, or one part of it, in the backend's
/// own terms.
///
/// The filesystem backend names a directory or a file by its absolute path; a
/// database backend could name a table and a row.
/// Callers show a location and compare it, and never interpret it.
///
/// A location also has a label: the short name a message uses for it, such as a
/// file name.
/// Two locations are the same location when their names are equal, whatever
/// their labels.
#[derive(Debug, Clone)]
pub struct StorageLocation {
    name: String,
    label: Option<String>,
}

impl StorageLocation {
    /// A location named by `name`, labelled by its name.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            label: None,
        }
    }

    /// A location named by `name`, which messages call `label`.
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

    /// The short name a message uses for the location.
    #[must_use]
    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }

    /// The location of a filesystem path, labelled by its file name.
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
