use crate::ConversationId;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Failed to serialize XML: {0}")]
    XmlSerialization(#[from] quick_xml::SeError),

    #[error("Invalid ID format: {0}")]
    InvalidIdFormat(String),

    #[error("Invalid ID: {0}")]
    Id(#[from] jp_id::Error),

    #[error("Invalid thread: {0}")]
    Thread(String),

    #[error("unknown conversation ID")]
    UnknownId(ConversationId),
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
