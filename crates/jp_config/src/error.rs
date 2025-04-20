pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Conversation error: {0}")]
    Conversation(#[from] jp_conversation::error::Error),

    #[error("Confique error: {0}")]
    Confique(#[from] confique::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Bool parse error: {0}")]
    ParseBool(#[from] std::str::ParseBoolError),

    #[error("Unknown config key: {key}\n\nAvailable keys:\n  - {}", available_keys.join("\n  - "))]
    UnknownConfigKey {
        key: String,
        available_keys: Vec<String>,
    },

    #[error("Invalid config value \"{value}\" for key {key}. Expected one of: {}", need.join(", "))]
    InvalidConfigValue {
        key: String,
        value: String,
        need: Vec<String>,
    },

    #[error("Model slug error: {0}")]
    ModelSlug(String),
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
