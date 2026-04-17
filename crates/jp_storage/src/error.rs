use camino::Utf8PathBuf;
use jp_conversation::ConversationId;

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Path is not a directory: {0}")]
    NotDir(Utf8PathBuf),

    #[error("Path is not a symlink: {0}")]
    NotSymlink(Utf8PathBuf),

    #[error("conversation error")]
    Conversation(#[from] jp_conversation::Error),

    #[error("configuration error")]
    Config(#[from] jp_config::error::Error),

    #[error("IO error")]
    Io(#[from] std::io::Error),

    #[error("invalid JSON data")]
    Json(#[from] serde_json::Error),

    #[error("invalid TOML data")]
    Toml(#[from] toml::de::Error),

    #[error("conversation not found: {0}")]
    ConversationNotFound(ConversationId),
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
