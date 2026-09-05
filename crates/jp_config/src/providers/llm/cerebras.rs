//! Cerebras API configuration.
//!
//! ```toml
//! [providers.llm.cerebras]
//! api_key_env = "CEREBRAS_API_KEY"
//! ```
//!
//! # Staying under the rate limit
//!
//! Cerebras meters tokens per minute across your whole organization, and
//! reserves a request's share when it admits the request rather than when it
//! finishes.
//! The reservation is `assistant.model.parameters.max_tokens`, capped at 16384
//! — which is also what it assumes when that setting is unset.
//!
//! So a short answer costs the same quota as a long one, and every parallel JP
//! session draws from the same bucket.
//! On a 30000 tokens-per-minute plan that is under two requests a minute.
//! Setting a smaller ceiling reserves less and fits more requests into the same
//! minute, at the cost of truncating answers that run past it:
//!
//! ```toml
//! [assistant.model.parameters]
//! max_tokens = 4096
//! ```

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt},
};

/// Cerebras API configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct CerebrasConfig {
    /// Environment variable that contains the API key.
    #[setting(default = "CEREBRAS_API_KEY")]
    pub api_key_env: String,

    /// The base URL to use for API requests.
    #[setting(default = "https://api.cerebras.ai")]
    pub base_url: String,
}

impl AssignKeyValue for PartialCerebrasConfig {
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

impl PartialConfigDelta for PartialCerebrasConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            api_key_env: delta_opt(self.api_key_env.as_ref(), next.api_key_env),
            base_url: delta_opt(self.base_url.as_ref(), next.base_url),
        }
    }
}

impl FillDefaults for PartialCerebrasConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            api_key_env: self.api_key_env.or(defaults.api_key_env),
            base_url: self.base_url.or(defaults.base_url),
        }
    }
}

impl ToPartial for CerebrasConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            api_key_env: partial_opt(&self.api_key_env, defaults.api_key_env),
            base_url: partial_opt(&self.base_url, defaults.base_url),
        }
    }
}
