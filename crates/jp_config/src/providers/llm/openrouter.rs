//! Openrouter API configuration.

use schematic::Config;

use crate::assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment};

/// Openrouter API configuration.
#[derive(Debug, Clone, Config)]
#[config(rename_all = "snake_case")]
pub struct OpenrouterConfig {
    /// Environment variable that contains the API key.
    #[setting(default = "OPENROUTER_API_KEY")]
    pub api_key_env: String,

    /// Application name sent to Openrouter.
    #[setting(default = "JP")]
    pub app_name: String,

    /// Optional HTTP referrer to send with requests.
    pub app_referrer: Option<String>,

    /// The base URL to use for API requests.
    #[setting(default = "https://openrouter.ai")]
    pub base_url: String,
}

impl AssignKeyValue for PartialOpenrouterConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "api_key_env" => self.api_key_env = kv.try_some_string()?,
            "app_name" => self.app_name = kv.try_some_string()?,
            "app_referrer" => self.app_referrer = kv.try_some_string()?,
            "base_url" => self.base_url = kv.try_some_string()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}
