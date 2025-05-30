use std::path::PathBuf;

use jp_conversation::model::SetParameterError;

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Conversation error: {0}")]
    Conversation(#[from] jp_conversation::error::Error),

    #[error("MCP error: {0}")]
    Mcp(#[from] jp_mcp::Error),

    #[error("Confique error: {0}")]
    Confique(#[from] confique::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Bool parse error: {0}")]
    ParseBool(#[from] std::str::ParseBoolError),

    #[error("Model parameter error: {0}")]
    Parameters(#[from] SetParameterError),

    #[error("Unknown config key: {key}\n\nAvailable keys:\n  - {}", available_keys.join("\n  - "))]
    UnknownConfigKey {
        key: String,
        available_keys: Vec<String>,
    },

    #[error("Invalid config value \"{value}\" for key {key}. Expected one of: {}", need.join(", "))]
    InvalidConfigValue {
        key: String,
        value: String,
        need: Vec<String>,
    },

    #[error("Model slug error: {0}")]
    ModelSlug(String),

    #[error("Invalid or missing file extension: {path}")]
    InvalidFileExtension { path: PathBuf },

    #[error("TOML error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("JSON error: {0}")]
    Json5(#[from] json5::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("Deserialization error: {0}")]
    Deserialize(#[from] serde::de::value::Error),
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
