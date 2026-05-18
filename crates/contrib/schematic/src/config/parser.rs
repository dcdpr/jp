use thiserror::Error;

/// Error for a single parse failure.
#[derive(Clone, Debug, Error)]
#[error("{message}")]
pub struct ParseError {
    /// Failure message.
    pub message: String,
}

impl ParseError {
    /// Create a new parse error with the provided message.
    pub fn new<T: AsRef<str>>(message: T) -> Self {
        ParseError {
            message: message.as_ref().to_owned(),
        }
    }
}

/// Error related to serde parsing.
#[derive(Debug, Error)]
#[error("{path}: {message}")]
pub struct ParserError {
    /// Failure message.
    pub message: String,

    /// Dot-notated path to the field that failed.
    pub path: String,
}
