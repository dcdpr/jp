use confique::Config as Confique;

use crate::error::Result;

/// Anthropic API configuration.
#[derive(Debug, Clone, PartialEq, Confique)]
pub struct Config {
    /// Environment variable that contains the API key.
    #[config(
        default = "ANTHROPIC_API_KEY",
        env = "JP_LLM_PROVIDER_ANTHROPIC_API_KEY_ENV"
    )]
    pub api_key_env: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_key_env: "ANTHROPIC_API_KEY".to_owned(),
        }
    }
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, path: &str, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            "api_key_env" => self.api_key_env = value.into(),
            _ => return crate::set_error(path, key),
        }

        Ok(())
    }
}
