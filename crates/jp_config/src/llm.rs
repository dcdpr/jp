pub mod model;
pub mod provider;

use std::str::FromStr;

use confique::Config as Confique;
pub use model::ProviderModelSlug;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// LLM configuration.
#[derive(Debug, Clone, Default, PartialEq, Confique)]
pub struct Config {
    /// Provider configuration.
    #[config(nested)]
    pub provider: provider::Config,

    /// Model configuration.
    #[config(nested)]
    pub model: model::Config,

    /// How the LLM should choose tools, if any are available.
    #[config(default = "auto", env = "JP_LLM_TOOL_CHOICE", deserialize_with = de_tool_choice)]
    pub tool_choice: ToolChoice,
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, path: &str, key: &str, value: impl Into<String>) -> Result<()> {
        let value: String = value.into();

        match key {
            _ if key.starts_with("provider.") => self.provider.set(path, &key[9..], value)?,
            _ if key.starts_with("model.") => self.model.set(path, &key[6..], value)?,
            "tool_choice" => self.tool_choice = value.parse()?,
            _ => return crate::set_error(path, key),
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(untagged, rename_all = "snake_case")]
pub enum ToolChoice {
    #[default]
    Auto,
    None,
    Required,
    Function(String),
}

impl FromStr for ToolChoice {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "auto" => Ok(Self::Auto),
            "none" => Ok(Self::None),
            "required" => Ok(Self::Required),
            _ if s.starts_with("fn:") && s.len() > 3 => Ok(Self::Function(s[3..].to_owned())),
            _ => Err(Error::InvalidConfigValue {
                key: s.to_string(),
                value: s.to_string(),
                need: vec![
                    "auto".to_owned(),
                    "none".to_owned(),
                    "required".to_owned(),
                    "fn:<name>".to_owned(),
                ],
            }),
        }
    }
}

fn de_tool_choice<'de, D>(deserializer: D) -> std::result::Result<ToolChoice, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let string: String = String::deserialize(deserializer)?;
    ToolChoice::from_str(&string).map_err(serde::de::Error::custom)
}
