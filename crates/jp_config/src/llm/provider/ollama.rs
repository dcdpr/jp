use confique::Config as Confique;

use crate::error::Result;

/// Ollama API configuration.
#[derive(Debug, Clone, PartialEq, Confique)]
pub struct Config {
    /// The base URL to use for API requests.
    #[config(
        default = "http://localhost:11434",
        env = "JP_LLM_PROVIDER_OLLAMA_BASE_URL"
    )]
    pub base_url: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:11434".to_owned(),
        }
    }
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, path: &str, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            "base_url" => self.base_url = value.into(),
            _ => return crate::set_error(path, key),
        }

        Ok(())
    }
}
