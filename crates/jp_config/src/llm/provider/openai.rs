use confique::Config as Confique;

use crate::error::Result;

/// Openai API configuration.
#[derive(Debug, Clone, Confique)]
pub struct Config {
    /// Environment variable that contains the API key.
    #[config(default = "OPENAI_API_KEY", env = "JP_LLM_PROVIDER_OPENAI_API_KEY_ENV")]
    pub api_key_env: String,

    /// The base URL to use for API requests.
    ///
    /// Used if `OPENAI_BASE_URL` is not set.
    #[config(
        default = "https://api.openai.com/v1",
        env = "JP_LLM_PROVIDER_OPENAI_BASE_URL"
    )]
    pub base_url: String,

    /// Environment variable that contains the API base URL key.
    #[config(
        default = "OPENAI_BASE_URL",
        env = "JP_LLM_PROVIDER_OPENAI_BASE_URL_ENV"
    )]
    pub base_url_env: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_key_env: "OPENAI_API_KEY".to_owned(),
            base_url: "https://api.openai.com/v1".to_owned(),
            base_url_env: "OPENAI_BASE_URL".to_owned(),
        }
    }
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, path: &str, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            "api_key_env" => self.api_key_env = value.into(),
            "base_url" => self.base_url = value.into(),
            "base_url_env" => self.base_url_env = value.into(),
            _ => return crate::set_error(path, key),
        }

        Ok(())
    }
}
