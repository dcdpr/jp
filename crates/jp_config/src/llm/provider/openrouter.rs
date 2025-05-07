use confique::Config as Confique;
use serde::Serialize;

use crate::error::Result;

/// Openrouter API configuration.
#[derive(Debug, Clone, Confique, Serialize)]
pub struct Config {
    /// Environment variable that contains the API key.
    #[config(
        default = "OPENROUTER_API_KEY",
        env = "JP_LLM_PROVIDER_OPENROUTER_API_KEY_ENV"
    )]
    pub api_key_env: String,

    /// Application name sent to Openrouter.
    #[config(default = "JP", env = "JP_LLM_PROVIDER_OPENROUTER_APP_NAME")]
    pub app_name: String,

    /// Optional HTTP referrer to send with requests.
    #[config(env = "JP_LLM_PROVIDER_OPENROUTER_APP_REFERRER")]
    pub app_referrer: Option<String>,

    /// The base URL to use for API requests.
    #[config(
        default = "https://openrouter.ai",
        env = "JP_LLM_PROVIDER_OPENROUTER_BASE_URL"
    )]
    pub base_url: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_key_env: "OPENROUTER_API_KEY".to_owned(),
            app_name: "JP".to_owned(),
            app_referrer: None,
            base_url: "https://openrouter.ai".to_owned(),
        }
    }
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            "api_key_env" => self.api_key_env = value.into(),
            "app_name" => self.app_name = value.into(),
            "app_referrer" => self.app_referrer = Some(value.into()),
            "base_url" => self.base_url = value.into(),
            _ => return crate::set_error(key),
        }

        Ok(())
    }
}
