//! Google API configuration.

use schematic::Config;

use crate::{
    assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment},
    delta::{delta_opt, PartialConfigDelta},
};

/// Google API configuration.
#[derive(Debug, Clone, Config)]
#[config(rename_all = "snake_case")]
pub struct GoogleConfig {
    /// Environment variable that contains the API key.
    #[setting(default = "GEMINI_API_KEY")]
    pub api_key_env: String,

    /// The base URL to use for API requests.
    #[setting(default = "https://generativelanguage.googleapis.com/v1beta")]
    pub base_url: String,
}

impl AssignKeyValue for PartialGoogleConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "api_key_env" => self.api_key_env = kv.try_some_string()?,
            "base_url" => self.base_url = kv.try_some_string()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialGoogleConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            api_key_env: delta_opt(self.api_key_env.as_ref(), next.api_key_env),
            base_url: delta_opt(self.base_url.as_ref(), next.base_url),
        }
    }
}
