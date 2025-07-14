use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub const CONFIG_FILE_EXTENSIONS: &[&str] = &["toml", "json", "json5", "yaml", "yml"];

#[derive(Debug, thiserror::Error)]
pub enum ConfigLoaderError {
    #[error("provided path is not a directory")]
    PathIsNotADirectory { got: PathBuf },

    #[error("config file not found")]
    NotFound {
        path: PathBuf,
        stem: String,
        extensions: Vec<String>,
    },

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
    pub recurse_stop_at: Option<PathBuf>,

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
    pub path: PathBuf,

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
    pub fn deserialize<T: for<'de> Deserialize<'de>>(
        &self,
    ) -> Result<T, Box<dyn std::error::Error + Send + Sync>> {
        match self.format {
            Format::Toml => toml::from_str(&self.content).map_err(Into::into),
            Format::Json => serde_json::from_str(&self.content).map_err(Into::into),
            Format::Json5 => json5::from_str(&self.content).map_err(Into::into),
            Format::Yaml => serde_yaml::from_str(&self.content).map_err(Into::into),
        }
    }

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
            Format::Toml => toml_edit::ser::to_string_pretty(&value)?,
            Format::Json => serde_json::to_string_pretty(&value)?,
            Format::Json5 => json5::to_string(&value)?,
            Format::Yaml => serde_yaml::to_string(&value)?,
        };

        Ok(())
    }

    /// Format the content of the configuration file, using the provided type.
    pub fn format_content<T>(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        self.content = match self.format {
            Format::Toml => toml::to_string_pretty(&self.deserialize::<T>()?)?,
            Format::Json => serde_json::to_string_pretty(&self.deserialize::<T>()?)?,
            Format::Json5 => json5::to_string(&self.deserialize::<T>()?)?,
            Format::Yaml => serde_yaml::to_string(&self.deserialize::<T>()?)?,
        };

        Ok(())
    }
}

/// A configuration file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    Toml,
    Json,
    Json5,
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
    pub fn as_str(&self) -> &'static str {
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
    pub fn load<P: AsRef<Path>>(&self, directory: P) -> Result<ConfigFile, ConfigLoaderError> {
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
