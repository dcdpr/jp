#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Missing prefix: {0}")]
    MissingPrefix(String),

    #[error("Invalid prefix, must be {0}: {1}")]
    InvalidPrefix(&'static str, String),

    #[error("Missing variant and target id")]
    MissingVariantAndTargetId,

    #[error("Missing variant")]
    MissingVariant,

    #[error("Invalid variant, must be [a-z]: {0}")]
    InvalidVariant(char),

    #[error("Unexpected variant, must be {0}: {1}")]
    UnexpectedVariant(char, char),

    #[error("Missing target ID")]
    MissingTargetId,

    #[error("Invalid timestamp format: {0}")]
    InvalidTimestamp(String),

    #[error("Missing global ID")]
    MissingGlobalId,

    #[error("Invalid global ID, must be [a-z]: {0}")]
    InvalidGlobalId(String),
}
