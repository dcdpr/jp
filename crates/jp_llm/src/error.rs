pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("missing env var: {0}")]
    MissingEnv(#[from] std::env::VarError),

    #[error("OpenRouter error: {0}")]
    OpenRouter(#[from] jp_openrouter::Error),

    #[error("Conversation error: {0}")]
    Conversation(#[from] jp_conversation::Error),

    // TODO: remove this
    #[error("{0}")]
    Other(String),
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
