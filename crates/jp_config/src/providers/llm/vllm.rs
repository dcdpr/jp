//! vLLM API configuration.
//!
//! A vLLM server speaks the OpenAI-compatible `/v1/chat/completions` dialect
//! and checks the Bearer token given to it with `--api-key`.
//!
//! ```toml
//! [providers.llm.vllm]
//! api_key_env = "VLLM_API_KEY"
//! base_url = "http://127.0.0.1:8000"
//! ```

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt},
};

/// vLLM API configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct VllmConfig {
    /// Environment variable that contains the API key.
    #[setting(default = "VLLM_API_KEY")]
    pub api_key_env: String,

    /// The base URL to use for API requests.
    ///
    /// The default is `http://127.0.0.1:8000`, which is the default URL for a
    /// vLLM server.
    #[setting(default = "http://127.0.0.1:8000")]
    pub base_url: String,
}

impl AssignKeyValue for PartialVllmConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "api_key_env" => self.api_key_env = kv.try_some_string()?,
            "base_url" => self.base_url = kv.try_some_string()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialVllmConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            api_key_env: delta_opt(self.api_key_env.as_ref(), next.api_key_env),
            base_url: delta_opt(self.base_url.as_ref(), next.base_url),
        }
    }
}

impl FillDefaults for PartialVllmConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            api_key_env: self.api_key_env.or(defaults.api_key_env),
            base_url: self.base_url.or(defaults.base_url),
        }
    }
}

impl ToPartial for VllmConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            api_key_env: partial_opt(&self.api_key_env, defaults.api_key_env),
            base_url: partial_opt(&self.base_url, defaults.base_url),
        }
    }
}
