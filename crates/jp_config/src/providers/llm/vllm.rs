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

use schematic::{Config, ConfigError};
use serde_json::Value;

pub use crate::providers::llm::{AuthEntry, AuthEntryParseError};
use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt},
    types::api_key_env::ApiKeyEnv,
    validate::Validator,
};

/// The configuration path the credential chain lives at.
const AUTH_KEY: &str = "providers.llm.vllm.auth";

/// vLLM API configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct VllmConfig {
    /// The credential chain used to authenticate requests, in fallback order.
    ///
    /// Defaults to `["api_key"]`.
    ///
    /// - `api_key`: The key `api_key_env` names.
    /// - `api_key:<name>`: The named key, when `api_key_env` maps several, for
    ///   example one per vLLM deployment.
    ///
    /// `api` is accepted as shorthand for the kind, and a bare name selects the
    /// key that answers to it.
    /// Names are case-sensitive.
    ///
    /// vLLM has no subscription plan, so a `subscription` entry is a resolution
    /// error.
    ///
    /// Entries are tried in order: when one cannot produce a usable credential,
    /// JP continues with the next.
    ///
    /// ```toml
    /// [providers.llm.vllm]
    /// auth = ["api_key:work", "api_key:personal"]
    /// ```
    #[setting(default = vec![AuthEntry::ApiKey(None)])]
    pub auth: Vec<AuthEntry>,

    /// Environment variable that contains the API key.
    ///
    /// A map names several keys, each selectable from the `auth` chain as
    /// `api_key:<name>`:
    ///
    /// ```toml
    /// api_key_env = { work = "WORK_VLLM_KEY", personal = "MY_VLLM_KEY" }
    /// ```
    #[setting(default = "VLLM_API_KEY")]
    pub api_key_env: ApiKeyEnv,

    /// The base URL to use for API requests.
    ///
    /// The default is `http://127.0.0.1:8000`, which is the default URL for a
    /// vLLM server.
    #[setting(default = "http://127.0.0.1:8000")]
    pub base_url: String,
}

impl Validator for VllmConfig {
    /// Rejects an empty or duplicate-carrying `auth` chain.
    ///
    /// Unrecognized entries are rejected earlier, when the value is parsed into
    /// an [`AuthEntry`].
    fn validate(&self) -> Result<(), ConfigError> {
        AuthEntry::validate_chain(&self.auth, AUTH_KEY)
    }
}

impl AssignKeyValue for PartialVllmConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("auth") => {
                kv.try_some_vec(&mut self.auth, |kv| match kv.value.into_value() {
                    Value::String(s) => s.parse::<AuthEntry>().map_err(Into::into),
                    value => Err(format!("expected a string, got {value}").into()),
                })?;
            }
            "api_key_env" => self.api_key_env = kv.try_some_object_or_from_str()?,
            "base_url" => self.base_url = kv.try_some_string()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialVllmConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            auth: delta_opt(self.auth.as_ref(), next.auth),
            api_key_env: delta_opt(self.api_key_env.as_ref(), next.api_key_env),
            base_url: delta_opt(self.base_url.as_ref(), next.base_url),
        }
    }
}

impl FillDefaults for PartialVllmConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            auth: self.auth.or(defaults.auth),
            api_key_env: self.api_key_env.or(defaults.api_key_env),
            base_url: self.base_url.or(defaults.base_url),
        }
    }
}

impl ToPartial for VllmConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            auth: partial_opt(&self.auth, defaults.auth),
            api_key_env: partial_opt(&self.api_key_env, defaults.api_key_env),
            base_url: partial_opt(&self.base_url, defaults.base_url),
        }
    }
}
