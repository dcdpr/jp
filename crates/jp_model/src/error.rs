pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid ID format: {0}")]
    InvalidIdFormat(String),

    #[error("Invalid provider ID: {0}")]
    InvalidProviderId(String),

    #[error("Invalid ID: {0}")]
    Id(#[from] jp_id::Error),

    #[error("ID missing")]
    MissingId,
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
