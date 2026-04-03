use std::fmt;

/// An error produced during parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub offset: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "parse error at offset {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for ParseError {}

/// An error produced during a merge operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeError {
    /// The document root is not a JSON object.
    RootNotObject,
    /// The source value is not a JSON object.
    SourceNotObject,
}

impl fmt::Display for MergeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootNotObject => write!(f, "document root is not a JSON object"),
            Self::SourceNotObject => write!(f, "source value is not a JSON object"),
        }
    }
}

impl std::error::Error for MergeError {}
