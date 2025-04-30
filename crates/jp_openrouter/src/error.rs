pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Request error: {0}")]
    Request(#[from] reqwest::Error),

    #[error("Stream processing error: {0}")]
    Stream(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("API error (status {}): {}", .code, .message)]
    Api { code: u16, message: String },

    #[error("client config error: {0}")]
    Config(String),
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
