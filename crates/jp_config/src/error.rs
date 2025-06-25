use std::path::PathBuf;

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Model error: {0}")]
    Model(#[from] jp_model::Error),

    #[error("MCP error: {0}")]
    Mcp(#[from] jp_mcp::Error),

    #[error(transparent)]
    Confique(#[from] confique::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Bool parse error: {0}")]
    ParseBool(#[from] std::str::ParseBoolError),

    #[error("Parse int error: {0}")]
    ParseInt(#[from] std::num::ParseIntError),

    #[error("Url parse error: {0}")]
    Url(#[from] url::ParseError),

    #[error("Unknown config key: {key}\n\nAvailable keys:\n  - {}", available_keys.join("\n  - "))]
    UnknownConfigKey {
        key: String,
        available_keys: Vec<String>,
    },

    #[error("Invalid config value \"{value}\" for key {key}. Expected one of: {}", need.join(", "))]
    InvalidConfigValueType {
        key: String,
        value: String,
        need: Vec<String>,
    },

    #[error("Unable to parse config value \"{value}\" for key {key}: {error}")]
    ValueParseError {
        key: String,
        value: String,
        error: String,
    },

    #[error("Model slug error: {0}")]
    ModelSlug(String),

    #[error(r#"Invalid or missing file extension: {path}, must be one of "json", "json5", "yaml", "yml" or "toml""#)]
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

    #[error("Failed to serialize XML: {0}")]
    XmlSerialization(#[from] quick_xml::SeError),
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
