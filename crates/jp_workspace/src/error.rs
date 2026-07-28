use camino::Utf8PathBuf;
use jp_storage::error::is_storage_full;

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid workspace ID: {0}")]
    Id(String),

    #[error("Storage error: {0}")]
    Storage(#[from] jp_storage::Error),

    #[error("Error loading workspace state")]
    Load(#[from] jp_storage::LoadError),

    #[error("Config error: {0}")]
    Config(#[from] jp_config::Error),

    #[error("Cannot persist workspace without storage")]
    MissingStorage,

    #[error("Failed to acquire lock on conversation {0}")]
    LockFailed(String),

    #[error("Cannot persist workspace without valid home directory")]
    MissingHome,

    #[error("Path is not a directory: {0}")]
    NotDir(Utf8PathBuf),

    #[error("{0} not found: {1}")]
    NotFound(&'static str, String),

    #[error("{target} already exists: {id}")]
    Exists { target: &'static str, id: String },

    #[error("Conversation error: {0}")]
    Conversation(#[from] jp_conversation::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    /// Whether the error means the filesystem is full.
    ///
    /// A caller that sees this should stop attempting writes rather than move
    /// on to the next one: none of them can succeed until space is freed.
    #[must_use]
    pub fn is_out_of_space(&self) -> bool {
        match self {
            Self::Storage(error) => error.is_out_of_space(),
            Self::Io(error) => is_storage_full(error),
            _ => false,
        }
    }

    pub fn not_found(target: &'static str, id: &impl ToString) -> Self {
        Self::NotFound(target, id.to_string())
    }

    pub fn exists(target: &'static str, id: &impl ToString) -> Self {
        Self::Exists {
            target,
            id: id.to_string(),
        }
    }
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
