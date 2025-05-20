use std::path::PathBuf;

use jp_conversation::ConversationId;

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid workspace ID: {0}")]
    Id(String),

    #[error("Cannot persist workspace without storage")]
    MissingStorage,

    #[error("Cannot persist workspace without valid home directory")]
    MissingHome,

    #[error("Path is not a directory: {0}")]
    NotDir(PathBuf),

    #[error("Path is not a symlink: {0}")]
    NotSymlink(PathBuf),

    #[error("{0} not found: {1}")]
    NotFound(&'static str, String),

    #[error("Cannot remove active conversation: {0}")]
    CannotRemoveActiveConversation(ConversationId),

    #[error("{target} already exists: {id}")]
    Exists { target: &'static str, id: String },

    #[error("Failed to persist storage: {src} -> {dst}: {error}")]
    AtomicReplaceFailed {
        src: PathBuf,
        dst: PathBuf,
        #[source]
        error: std::io::Error,
    },

    #[error("Conversation error: {0}")]
    Conversation(#[from] jp_conversation::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

impl Error {
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
