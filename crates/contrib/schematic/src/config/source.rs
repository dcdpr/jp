use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::error::ConfigError;

/// Source from which to load a configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Source {
    /// Inline code snippet of the configuration.
    Code { path: PathBuf, code: String },

    /// File system path to the configuration.
    File { path: PathBuf, required: bool },
}

impl Source {
    /// Create a new code snippet source.
    pub fn code<T: TryInto<String>, P: TryInto<PathBuf>>(
        code: T,
        path: P,
    ) -> Result<Source, ConfigError> {
        let path: PathBuf = path.try_into().map_err(|_| ConfigError::InvalidFile)?;
        let code: String = code.try_into().map_err(|_| ConfigError::InvalidCode)?;

        Ok(Source::Code { path, code })
    }

    /// Create a new file source with the provided path.
    pub fn file<P: TryInto<PathBuf>>(path: P, required: bool) -> Result<Source, ConfigError> {
        let path: PathBuf = path.try_into().map_err(|_| ConfigError::InvalidFile)?;

        Ok(Source::File { path, required })
    }

    /// Return a file extension (without period) for the source if one is available.
    #[must_use]
    pub fn get_file_ext(&self) -> Option<&str> {
        match self {
            Self::Code { path, .. } | Self::File { path, .. } => {
                path.extension().and_then(|name| name.to_str())
            }
        }
    }

    /// Return a file name for the source.
    #[must_use]
    pub fn get_file_name(&self) -> &str {
        match self {
            Self::Code { path, .. } | Self::File { path, .. } => path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("unknown"),
        }
    }

    /// Return the source as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Source::Code { path, .. } | Source::File { path, .. } => {
                path.to_str().unwrap_or_default()
            }
        }
    }
}

/// Parses a source into a specific format.
pub trait SourceFormat<T: DeserializeOwned> {
    /// Should this instance parse the provided source?
    fn should_parse(&self, source: &Source) -> bool;

    /// Parse the source contents and return the deserialized value.
    fn parse(
        &self,
        source: &Source,
        content: &str,
        cache_path: Option<&Path>,
    ) -> Result<T, ConfigError>;
}
