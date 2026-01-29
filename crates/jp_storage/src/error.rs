use camino::Utf8PathBuf;

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Path is not a directory: {0}")]
    NotDir(Utf8PathBuf),

    #[error("Path is not a symlink: {0}")]
    NotSymlink(Utf8PathBuf),

    #[error("Conversation error: {0}")]
    Conversation(#[from] jp_conversation::Error),

    #[error("Config error: {0}")]
    Config(#[from] jp_config::error::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("TOML error: {0}")]
    Toml(#[from] toml::de::Error),
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
