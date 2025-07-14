use std::path::PathBuf;

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Model error")]
    Model(#[from] jp_model::Error),

    #[error("MCP error")]
    Mcp(#[from] jp_mcp::Error),

    #[error(transparent)]
    Confique(#[from] confique::Error),

    #[error("IO error")]
    Io(#[from] std::io::Error),

    #[error("Bool parse error")]
    ParseBool(#[from] std::str::ParseBoolError),

    #[error("Parse int error")]
    ParseInt(#[from] std::num::ParseIntError),

    #[error("Parse float error")]
    ParseFloat(#[from] std::num::ParseFloatError),

    #[error("Url parse error")]
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

    #[error("Model slug error")]
    ModelSlug(String),

    #[error("Config file not found: {0}")]
    MissingConfigFile(PathBuf),

    #[error(r#"Invalid or missing file extension: {path}, must be one of "json", "json5", "yaml", "yml" or "toml""#)]
    InvalidFileExtension { path: PathBuf },

    #[error("TOML error")]
    Toml(#[from] toml::de::Error),

    #[error("JSON error")]
    Json5(#[from] json5::Error),

    #[error("JSON error")]
    Json(#[from] serde_json::Error),

    #[error("YAML error")]
    Yaml(#[from] serde_yaml::Error),

    #[error("Deserialization error")]
    Deserialize(#[from] serde::de::value::Error),

    #[error("Failed to serialize XML")]
    XmlSerialization(#[from] quick_xml::SeError),

    #[error("Config path not found: {0}")]
    ConfigNotFound(String),
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
