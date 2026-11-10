use std::io;

use camino::Utf8PathBuf;
use jp_conversation::ConversationId;

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Path is not a directory: {0}")]
    NotDir(Utf8PathBuf),

    #[error("conversation error")]
    Conversation(#[from] jp_conversation::Error),

    #[error("configuration error")]
    Config(#[from] jp_config::error::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// A write to `path` failed.
    ///
    /// `std::io::Error` carries no path, so a bare I/O failure cannot tell the
    /// user which file it was.
    #[error("failed to write {path}")]
    WriteFailed {
        path: Utf8PathBuf,
        #[source]
        source: io::Error,
    },

    /// Renaming `from` to `to` failed.
    ///
    /// Carries both paths because `source` can refer to either: for example,
    /// `NotFound` when the temporary `from` file was removed before the rename,
    /// even though `to` still exists.
    #[error("failed to rename {from} to {to}")]
    RenameFailed {
        from: Utf8PathBuf,
        to: Utf8PathBuf,
        #[source]
        source: io::Error,
    },

    /// The filesystem holding `path` has no room left.
    ///
    /// Separate from [`Error::WriteFailed`] because it is the one write failure
    /// with an obvious user action, and because no retry can succeed until
    /// space is freed.
    #[error("no space left on device while writing {path}")]
    OutOfSpace {
        path: Utf8PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("invalid JSON data")]
    Json(#[from] serde_json::Error),

    #[error("invalid TOML data")]
    Toml(#[from] toml::de::Error),

    #[error("conversation not found: {0}")]
    ConversationNotFound(ConversationId),
}

impl Error {
    /// Classify a failed write to `path`.
    ///
    /// Yields [`Error::OutOfSpace`] when the operating system reported a full
    /// filesystem, and [`Error::WriteFailed`] otherwise.
    #[must_use]
    pub fn write_failed(path: impl Into<Utf8PathBuf>, source: io::Error) -> Self {
        let path = path.into();
        if is_storage_full(&source) {
            Self::OutOfSpace { path, source }
        } else {
            Self::WriteFailed { path, source }
        }
    }

    /// Classify a failed rename of `from` to `to`.
    ///
    /// Yields [`Error::OutOfSpace`] for `to` when the operating system reports
    /// a full filesystem, and [`Error::RenameFailed`] otherwise.
    #[must_use]
    pub fn rename_failed(
        from: impl Into<Utf8PathBuf>,
        to: impl Into<Utf8PathBuf>,
        source: io::Error,
    ) -> Self {
        let to = to.into();
        if is_storage_full(&source) {
            Self::OutOfSpace { path: to, source }
        } else {
            Self::RenameFailed {
                from: from.into(),
                to,
                source,
            }
        }
    }
}

/// Whether an OS error reports a full filesystem.
///
/// Recognises both a raw OS error (`ENOSPC`, `ERROR_DISK_FULL`) and an error
/// constructed directly from the kind, since `std` maps the platform codes onto
/// [`io::ErrorKind::StorageFull`].
fn is_storage_full(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::StorageFull
}

#[cfg(test)]
impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        if std::mem::discriminant(self) != std::mem::discriminant(other) {
            return false;
        }

        // Good enough for testing purposes
        format!("{self:?}") == format!("{other:?}")
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
