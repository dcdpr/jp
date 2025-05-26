pub mod model;
pub mod provider;

use std::str::FromStr;

use confique::Config as Confique;
use jp_mcp::tool::ToolChoice;
use serde::Deserialize;

use crate::error::Result;

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
    #[config(env = "JP_LLM_TOOL_CHOICE", deserialize_with = de_tool_choice)]
    pub tool_choice: Option<ToolChoice>,
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, path: &str, key: &str, value: impl Into<String>) -> Result<()> {
        let value: String = value.into();

        match key {
            _ if key.starts_with("provider.") => self.provider.set(path, &key[9..], value)?,
            _ if key.starts_with("model.") => self.model.set(path, &key[6..], value)?,
            "tool_choice" => self.tool_choice = Some(value.parse()?),
            _ => return crate::set_error(path, key),
        }

        Ok(())
    }
}

fn de_tool_choice<'de, D>(deserializer: D) -> std::result::Result<ToolChoice, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let string: String = String::deserialize(deserializer)?;
    ToolChoice::from_str(&string).map_err(serde::de::Error::custom)
}
