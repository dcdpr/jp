use confique::Config as Confique;

use crate::error::Result;

/// Google API configuration.
#[derive(Debug, Clone, PartialEq, Confique)]
pub struct Config {
    /// Environment variable that contains the API key.
    #[config(default = "GEMINI_API_KEY", env = "JP_LLM_PROVIDER_GOOGLE_API_KEY_ENV")]
    pub api_key_env: String,

    /// The base URL to use for API requests.
    #[config(
        default = "https://generativelanguage.googleapis.com/v1beta",
        env = "JP_LLM_PROVIDER_GOOGLE_BASE_URL"
    )]
    pub base_url: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_key_env: "GEMINI_API_KEY".to_owned(),
            base_url: "https://generativelanguage.googleapis.com/v1beta".to_owned(),
        }
    }
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, path: &str, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            "api_key_env" => self.api_key_env = value.into(),
            "base_url" => self.base_url = value.into(),
            _ => return crate::set_error(path, key),
        }

        Ok(())
    }
}
