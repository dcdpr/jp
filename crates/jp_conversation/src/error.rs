//! Error variants for the `conversation` module.

use crate::{ConversationId, stream::StreamError};

/// A result type for the `conversation` module.
pub type Result<T> = std::result::Result<T, Error>;

/// Error type for the `conversation` module.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Failed to serialize XML.
    #[error("Failed to serialize XML: {0}")]
    XmlSerialization(#[from] quick_xml::SeError),

    /// Invalid ID format.
    #[error("Invalid ID format: {0}")]
    InvalidIdFormat(String),

    /// Invalid ID.
    #[error("Invalid ID: {0}")]
    Id(#[from] jp_id::Error),

    /// Invalid thread.
    #[error("Invalid thread: {0}")]
    Thread(String),

    /// Unknown conversation ID.
    #[error("unknown conversation ID")]
    UnknownId(ConversationId),

    /// Configuration error.
    #[error(transparent)]
    Config(#[from] jp_config::Error),

    /// Stream error.
    #[error(transparent)]
    Stream(#[from] StreamError),
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
