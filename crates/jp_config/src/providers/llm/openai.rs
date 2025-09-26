//! `OpenAI` API configuration.

use schematic::Config;

use crate::{
    assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment},
    delta::{delta_opt, PartialConfigDelta},
    partial::{partial_opt, ToPartial},
};

/// `OpenAI` API configuration.
#[derive(Debug, Clone, Config)]
#[config(rename_all = "snake_case")]
pub struct OpenaiConfig {
    /// Environment variable that contains the API key.
    #[setting(default = "OPENAI_API_KEY")]
    pub api_key_env: String,

    /// The base URL to use for API requests.
    ///
    /// Used if `OPENAI_BASE_URL` is not set.
    #[setting(default = "https://api.openai.com")]
    pub base_url: String,

    /// Environment variable that contains the API base URL key.
    #[setting(default = "OPENAI_BASE_URL")]
    pub base_url_env: String,
}

impl AssignKeyValue for PartialOpenaiConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "api_key_env" => self.api_key_env = kv.try_some_string()?,
            "base_url" => self.base_url = kv.try_some_string()?,
            "base_url_env" => self.base_url_env = kv.try_some_string()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialOpenaiConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            api_key_env: delta_opt(self.api_key_env.as_ref(), next.api_key_env),
            base_url: delta_opt(self.base_url.as_ref(), next.base_url),
            base_url_env: delta_opt(self.base_url_env.as_ref(), next.base_url_env),
        }
    }
}

impl ToPartial for OpenaiConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            api_key_env: partial_opt(&self.api_key_env, defaults.api_key_env),
            base_url: partial_opt(&self.base_url, defaults.base_url),
            base_url_env: partial_opt(&self.base_url_env, defaults.base_url_env),
        }
    }
}
