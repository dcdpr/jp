//! Where a storage backend holds a conversation.

use std::fmt;

use camino::Utf8Path;
use serde::Serialize;

/// Where a backend holds a conversation, or one part of it, in the backend's
/// own terms.
///
/// The filesystem backend names a directory or a file by its absolute path; a
/// database backend could name a table and a row.
/// Callers show a location and compare it, and never interpret it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct StorageLocation(String);

impl StorageLocation {
    /// A location named by `name`.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The location of a filesystem path.
    pub(crate) fn of_path(path: &Utf8Path) -> Self {
        Self(path.as_str().to_owned())
    }
}

impl fmt::Display for StorageLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
