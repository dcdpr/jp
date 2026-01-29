//! Configuration file loader.

use std::{borrow::Cow, env};

use camino::{Utf8Path, Utf8PathBuf};
use clean_path::clean;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::{Error, PartialAppConfig};

/// Application name for configuration file storage paths.
const APPLICATION: &str = "jp";

/// Valid configuration file extensions.
pub const CONFIG_FILE_EXTENSIONS: &[&str] = &["toml", "json", "json5", "yaml", "yml"];

/// Environment variable used to specify the path to the global configuration
const GLOBAL_CONFIG_ENV_VAR: &str = "JP_GLOBAL_CONFIG_FILE";

/// Configuration loader error.
#[derive(Debug, thiserror::Error)]
pub enum ConfigLoaderError {
    /// Provided path is not a directory.
    #[error("provided path is not a directory")]
    PathIsNotADirectory {
        /// The path which is not a directory.
        got: Utf8PathBuf,
    },

    /// Configuration file not found.
    #[error("config file not found")]
    NotFound {
        /// The path to the configuration file.
        path: Utf8PathBuf,

        /// The file stem which was searched for.
        stem: String,

        /// The extensions which were searched for.
        extensions: Vec<String>,
    },

    /// IO error.
    #[error("IO error")]
    Io(#[from] std::io::Error),
}

/// A configuration file loader.
pub struct ConfigLoader {
    /// file stem to search for.
    ///
    /// This is the file name without the extension (e.g. `config` or `.jp`),
    /// the extension is fixed to a list of valid extensions. See
    /// [`CONFIG_FILE_EXTENSIONS`].
    pub file_stem: Cow<'static, str>,

    /// Whether to recurse upwards from the provided path to find a
    /// configuration file.
    pub recurse_up: bool,

    /// The final path to search for a configuration file, if `recurse_up` is
    /// enabled.
    pub recurse_stop_at: Option<Utf8PathBuf>,

    /// Whether to create a new configuration file if none is found.
    ///
    /// If `Some`, the provided format is used to create a new configuration
    /// file, attached to the provided `path` in [`ConfigLoader::load`], and the
    /// configured `file_stem`.
    pub create_if_missing: Option<Format>,
}

impl Default for ConfigLoader {
    fn default() -> Self {
        Self {
            file_stem: "config".into(),
            recurse_up: false,
            recurse_stop_at: None,
            create_if_missing: None,
        }
    }
}

/// A configuration file.
#[derive(Debug)]
pub struct ConfigFile {
    /// The path to the file.
    pub path: Utf8PathBuf,

    /// The format of the file.
    pub format: Format,

    /// The file content.
    pub content: String,
}

impl ConfigFile {
    /// Deserialize the file content into a valid type.
    ///
    /// Returns an error if the file content could not be deserialized into the
    /// provided type `T`.
    fn deserialize<T: for<'de> Deserialize<'de>>(
        &self,
    ) -> Result<T, Box<dyn std::error::Error + Send + Sync>> {
        match self.format {
            Format::Toml => toml::from_str(&self.content).map_err(Into::into),
            Format::Json => serde_json::from_str(&self.content).map_err(Into::into),
            Format::Json5 => serde_json5::from_str(&self.content).map_err(Into::into),
            Format::Yaml => serde_yaml::from_str(&self.content).map_err(Into::into),
        }
    }

    /// Edit the file content using the provided function.
    ///
    /// # Errors
    ///
    /// Returns an error if the file content could not be deserialized into the
    /// provided type `T`, or if the function returns an error.
    pub fn edit_content<T>(
        &mut self,
        f: impl FnOnce(&mut T) -> Result<(), Box<dyn std::error::Error + Send + Sync>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let mut value = self.deserialize::<T>()?;
        f(&mut value)?;

        self.content = match self.format {
            Format::Toml => toml::ser::to_string_pretty(&value)?,
            Format::Json => serde_json::to_string_pretty(&value)?,
            Format::Json5 => serde_json5::to_string(&value)?,
            Format::Yaml => serde_yaml::to_string(&value)?,
        };

        Ok(())
    }

    /// Format the content of the configuration file, using the provided type.
    ///
    /// # Errors
    ///
    /// Returns an error if the file content could not be deserialized into the
    /// provided type `T`.
    pub fn format_content<T>(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        self.content = match self.format {
            Format::Toml => toml::to_string_pretty(&self.deserialize::<T>()?)?,
            Format::Json => serde_json::to_string_pretty(&self.deserialize::<T>()?)?,
            Format::Json5 => serde_json5::to_string(&self.deserialize::<T>()?)?,
            Format::Yaml => serde_yaml::to_string(&self.deserialize::<T>()?)?,
        };

        Ok(())
    }
}

/// A configuration file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// TOML format.
    Toml,

    /// JSON format.
    Json,

    /// JSON5 format.
    Json5,

    /// YAML format.
    Yaml,
}

impl Format {
    /// Get the format from a file extension.
    #[must_use]
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "toml" => Some(Self::Toml),
            "json" => Some(Self::Json),
            "json5" => Some(Self::Json5),
            "yaml" | "yml" => Some(Self::Yaml),
            _ => None,
        }
    }

    /// Get the file extension as a static string slice.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Toml => "toml",
            Self::Json => "json",
            Self::Json5 => "json5",
            Self::Yaml => "yaml",
        }
    }
}

impl ConfigLoader {
    /// Load the closest configuration file to `path`, if any, or create a new
    /// one if configured to do so.
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration file could not be found, or if the
    /// file could not be loaded.
    pub fn load<P: AsRef<Utf8Path>>(&self, directory: P) -> Result<ConfigFile, ConfigLoaderError> {
        let directory = directory.as_ref();

        // Directory must exist.
        if !directory.is_dir() {
            return Err(ConfigLoaderError::PathIsNotADirectory {
                got: directory.to_path_buf(),
            });
        }

        // Create the path to the file using the directory and stem.
        let mut path = directory.join(self.file_stem.as_ref());

        // Iterate all valid file extensions.
        for ext in CONFIG_FILE_EXTENSIONS {
            // Check if the file exists with the directory, stem and
            // extension.
            path.set_extension(ext);
            if path.is_file() {
                // Check if the extension is valid.
                if let Some(format) = Format::from_extension(ext) {
                    let content = std::fs::read_to_string(&path)?;

                    // File found.
                    return Ok(ConfigFile {
                        path,
                        format,
                        content,
                    });
                }
            }
        }

        // If recursion is enabled, go up one level.
        if self.recurse_up
            && self
                .recurse_stop_at
                .as_deref()
                .is_none_or(|root| root != directory)
            && let Some(directory) = directory.parent()
        {
            return self.load(directory);
        }

        // If `create_if_missing` is enabled, create a new file.
        if let Some(format) = self.create_if_missing {
            std::fs::create_dir_all(directory)?;
            let mut path = directory.join(self.file_stem.as_ref());
            path.set_extension(format.as_str());
            let content = match format {
                Format::Toml | Format::Yaml => "",
                Format::Json | Format::Json5 => "{}",
            };

            std::fs::write(&path, content)?;

            return Ok(ConfigFile {
                path,
                format,
                content: content.to_owned(),
            });
        }

        // No file found.
        Err(ConfigLoaderError::NotFound {
            path: directory.to_path_buf(),
            stem: self.file_stem.to_string(),
            extensions: CONFIG_FILE_EXTENSIONS
                .iter()
                .map(ToString::to_string)
                .collect(),
        })
    }
}

/// Get the path to user the global config directory, if it exists.
#[must_use]
pub fn user_global_config_path(home: Option<&Utf8Path>) -> Option<Utf8PathBuf> {
    env::var(GLOBAL_CONFIG_ENV_VAR)
        .ok()
        .and_then(|path| expand_tilde(path, home))
        .and_then(|path| Utf8PathBuf::from_path_buf(clean(path)).ok())
        .inspect(|path| {
            debug!(
                path = path.as_str(),
                "Custom global configuration file path configured."
            );
        })
        .or_else(|| {
            ProjectDirs::from("", "", APPLICATION)
                .map(|p| p.config_dir().to_path_buf())
                .and_then(|path| Utf8PathBuf::from_path_buf(clean(path)).ok())
        })
}

/// Expand tilde in path to home directory
///
/// If no tilde is found, returns `Some` with the original path. If a tilde is
/// found, but no home directory is set, returns `None`.
pub fn expand_tilde<T: AsRef<str>>(path: impl AsRef<str>, home: Option<T>) -> Option<Utf8PathBuf> {
    if path.as_ref().starts_with('~') {
        return home.map(|home| Utf8PathBuf::from(path.as_ref().replacen('~', home.as_ref(), 1)));
    }

    Some(Utf8PathBuf::from(path.as_ref()))
}

/// Load a partial configuration, with optional fallback.
///
/// # Errors
///
/// Returns an error if merging the partials fails, which returns a
/// [`schematic::MergeError`].
pub fn load_partial(
    mut prev: PartialAppConfig,
    next: PartialAppConfig,
) -> Result<PartialAppConfig, Error> {
    use schematic::PartialConfig as _;

    prev.merge(&(), next)?;
    Ok(prev)
}

#[cfg(test)]
mod tests {
    use test_log::test;

    use super::*;

    #[test]
    fn test_expand_tilde() {
        struct TestCase {
            path: &'static str,
            home: Option<&'static str>,
            expected: Option<&'static str>,
        }

        let cases = vec![
            ("no tilde with home", TestCase {
                path: "no/tilde/here",
                home: Some("/tmp"),
                expected: Some("no/tilde/here"),
            }),
            ("no tilde missing home", TestCase {
                path: "no/tilde/here",
                home: None,
                expected: Some("no/tilde/here"),
            }),
            ("tilde path with home", TestCase {
                path: "~/subdir",
                home: Some("/tmp"),
                expected: Some("/tmp/subdir"),
            }),
            ("only tilde with home", TestCase {
                path: "~",
                home: Some("/tmp"),
                expected: Some("/tmp"),
            }),
            ("tilde missing home", TestCase {
                path: "~",
                home: None,
                expected: None,
            }),
        ];

        for (name, case) in cases {
            assert_eq!(
                expand_tilde(case.path, case.home),
                case.expected.map(Utf8PathBuf::from),
                "Failed test case: {name}"
            );
        }
    }
}
