use std::{fmt::Display, path::PathBuf};

use thiserror::Error;

use super::{merger::MergeError, parser::ParserError};

/// All configuration based errors.
#[derive(Error, Debug)]
pub enum ConfigError {
    #[error(transparent)]
    Handler(#[from] Box<HandlerError>),

    #[error(transparent)]
    Merge(#[from] Box<MergeError>),

    #[error("Invalid fallback variant {}, unable to parse type.", .0)]
    EnumInvalidFallback(String),

    #[error("Unknown enum variant {}.", .0)]
    EnumUnknownVariant(String),

    #[error("Invalid code block used as a source.")]
    InvalidCode,

    #[error("Invalid default value. {0}")]
    InvalidDefaultValue(String),

    #[error("Missing required value for field {}.", .fields.join("."))]
    MissingRequired { fields: Vec<String> },

    #[error("Invalid file path used as a source.")]
    InvalidFile,

    #[error("File path {} does not exist.", .0.display())]
    MissingFile(PathBuf),

    #[error(
        "Unable to parse {} as there's no matching source format for extension {}.", .src, .ext
    )]
    NoMatchingFormat { src: String, ext: String },

    #[error("Failed to read file {}.", .path.display())]
    ReadFileFailed {
        path: PathBuf,
        #[source]
        error: Box<std::io::Error>,
    },

    #[cfg(feature = "json")]
    #[error("Failed to strip comments from {}.", .file)]
    JsonStripCommentsFailed {
        file: String,
        #[source]
        error: Box<std::io::Error>,
    },

    #[error("Failed to parse {}.", .location)]
    Parser {
        location: String,

        #[source]
        error: Box<ParserError>,
    },
}

impl ConfigError {
    /// Return a full error string, including the source-chain inner messages
    /// concatenated onto the top-level error.
    #[must_use]
    pub fn to_full_string(&self) -> String {
        let mut message = self.to_string();
        let mut push_end = || {
            if !message.ends_with('\n') {
                if !message.ends_with('.') && !message.ends_with(':') {
                    message.push('.');
                }
                message.push(' ');
            }
        };

        match self {
            ConfigError::ReadFileFailed { error: inner, .. } => {
                push_end();
                message.push_str(&inner.to_string());
            }
            ConfigError::Parser { error: inner, .. } => {
                push_end();
                message.push_str(&inner.to_string());
            }
            _ => {}
        }

        message.trim().to_string()
    }
}

impl From<HandlerError> for ConfigError {
    fn from(error: HandlerError) -> ConfigError {
        ConfigError::Handler(Box::new(error))
    }
}

impl From<MergeError> for ConfigError {
    fn from(error: MergeError) -> ConfigError {
        ConfigError::Merge(Box::new(error))
    }
}

impl From<ParserError> for ConfigError {
    fn from(error: ParserError) -> ConfigError {
        ConfigError::Parser {
            location: String::new(),
            error: Box::new(error),
        }
    }
}

/// Error for handler functions.
#[derive(Error, Debug)]
#[error("{0}")]
pub struct HandlerError(pub String);

impl HandlerError {
    pub fn new<T: Display>(message: T) -> Self {
        Self(message.to_string())
    }
}
