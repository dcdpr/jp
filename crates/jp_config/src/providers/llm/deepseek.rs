//! Deepseek API configuration.

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    partial::{ToPartial, partial_opt},
};

/// Deepseek API configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct DeepseekConfig {
    /// Environment variable that contains the API key.
    #[setting(default = "DEEPSEEK_API_KEY")]
    pub api_key_env: String,

    /// The base URL to use for API requests.
    #[setting(default = "https://api.deepseek.com")]
    pub base_url: String,
}

impl AssignKeyValue for PartialDeepseekConfig {
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

impl PartialConfigDelta for PartialDeepseekConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            api_key_env: delta_opt(self.api_key_env.as_ref(), next.api_key_env),
            base_url: delta_opt(self.base_url.as_ref(), next.base_url),
        }
    }
}

impl ToPartial for DeepseekConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            api_key_env: partial_opt(&self.api_key_env, defaults.api_key_env),
            base_url: partial_opt(&self.base_url, defaults.base_url),
        }
    }
}
