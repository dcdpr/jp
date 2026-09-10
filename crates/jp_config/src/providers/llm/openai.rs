//! `OpenAI` API configuration.
//!
//! Requests reach `OpenAI` through one of two hosts, chosen by the credential
//! the `auth` chain lands on: `base_url` for an API key, `codex_base_url` for a
//! stored `ChatGPT` subscription profile.
//!
//! ```toml
//! [providers.llm.openai]
//! auth = ["subscription:personal", "api_key"]
//! ```

use schematic::{Config, ConfigError};

// Re-exported so `providers.llm.openai`'s own chain type is reachable alongside
// its config, though the grammar itself is shared.
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
const AUTH_KEY: &str = "providers.llm.openai.auth";

/// `OpenAI` API configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct OpenaiConfig {
    /// The credential chain used to authenticate requests, in fallback order.
    ///
    /// Defaults to `["api_key"]`.
    ///
    /// Each entry selects a credential source:
    ///
    /// - `api_key`: Metered billing, using the key `api_key_env` names, sent to
    ///   `base_url`.
    /// - `api_key:<name>`: Metered billing with the named key, when
    ///   `api_key_env` maps several.
    /// - `subscription`: A plan's allowance, using the sole stored credential.
    ///   Log in with `jp provider auth login llm.openai`.
    /// - `subscription:<name>`: The named stored credential.
    ///
    /// `api` and `sub` are accepted as shorthand for the two kinds.
    /// Names are case-sensitive.
    ///
    /// A `subscription` entry bills against a `ChatGPT` Plus/Pro allowance, and
    /// its requests go to `codex_base_url` instead of `base_url`.
    ///
    /// Entries are tried in order: when one cannot produce a usable credential,
    /// JP continues with the next.
    /// Listing `api_key` after subscription profiles authorizes continuing on
    /// per-token API billing when the subscription allowance is exhausted.
    ///
    /// This list replaces the one from earlier config layers as a whole; it
    /// never appends.
    ///
    /// ```toml
    /// [providers.llm.openai]
    /// auth = ["subscription:personal", "api_key"]
    /// ```
    #[setting(default = vec![AuthEntry::ApiKey(None)])]
    pub auth: Vec<AuthEntry>,

    /// Environment variable that contains the API key.
    ///
    /// A map names several keys, each selectable from the `auth` chain as
    /// `api_key:<name>`:
    ///
    /// ```toml
    /// api_key_env = { work = "WORK_OPENAI_KEY", personal = "MY_OPENAI_KEY" }
    /// ```
    #[setting(default = "OPENAI_API_KEY")]
    pub api_key_env: ApiKeyEnv,

    /// The base URL to use for API requests.
    ///
    /// Used if `OPENAI_BASE_URL` is not set.
    #[setting(default = "https://api.openai.com")]
    pub base_url: String,

    /// Environment variable that contains the API base URL key.
    ///
    /// If set, the value of this environment variable will override `base_url`.
    #[setting(default = "OPENAI_BASE_URL")]
    pub base_url_env: String,

    /// The base URL subscription requests are sent to.
    ///
    /// Used by `subscription` entries in the `auth` chain, which bill against a
    /// `ChatGPT` plan's allowance rather than per token.
    /// That host serves the responses endpoint at `/responses` rather than
    /// `/v1/responses`, and JP adjusts the path accordingly.
    ///
    /// Used if `JP_OPENAI_CODEX_BASE_URL` is not set.
    #[setting(default = "https://chatgpt.com/backend-api/codex")]
    pub codex_base_url: String,

    /// Environment variable that contains the subscription base URL.
    ///
    /// If set, the value of this environment variable overrides
    /// `codex_base_url`.
    /// Point it at a local recorder to exercise the subscription request path
    /// without reaching `OpenAI`.
    #[setting(default = "JP_OPENAI_CODEX_BASE_URL")]
    pub codex_base_url_env: String,
}

impl Validator for OpenaiConfig {
    /// Rejects an empty or duplicate-carrying `auth` chain.
    ///
    /// Unrecognized entries are rejected earlier, when the value is parsed into
    /// an [`AuthEntry`].
    fn validate(&self) -> Result<(), ConfigError> {
        AuthEntry::validate_chain(&self.auth, AUTH_KEY)
    }
}

impl AssignKeyValue for PartialOpenaiConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("auth") => {
                kv.try_some_vec(&mut self.auth, |kv| match kv.value.into_value() {
                    serde_json::Value::String(s) => s.parse().map_err(Into::into),
                    value => Err(format!("expected a string, got {value}").into()),
                })?;
            }
            "api_key_env" => self.api_key_env = kv.try_some_object_or_from_str()?,
            "base_url" => self.base_url = kv.try_some_string()?,
            "base_url_env" => self.base_url_env = kv.try_some_string()?,
            "codex_base_url" => self.codex_base_url = kv.try_some_string()?,
            "codex_base_url_env" => self.codex_base_url_env = kv.try_some_string()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialOpenaiConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            auth: delta_opt(self.auth.as_ref(), next.auth),
            api_key_env: delta_opt(self.api_key_env.as_ref(), next.api_key_env),
            base_url: delta_opt(self.base_url.as_ref(), next.base_url),
            base_url_env: delta_opt(self.base_url_env.as_ref(), next.base_url_env),
            codex_base_url: delta_opt(self.codex_base_url.as_ref(), next.codex_base_url),
            codex_base_url_env: delta_opt(
                self.codex_base_url_env.as_ref(),
                next.codex_base_url_env,
            ),
        }
    }
}

impl FillDefaults for PartialOpenaiConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            auth: self.auth.or(defaults.auth),
            api_key_env: self.api_key_env.or(defaults.api_key_env),
            base_url: self.base_url.or(defaults.base_url),
            base_url_env: self.base_url_env.or(defaults.base_url_env),
            codex_base_url: self.codex_base_url.or(defaults.codex_base_url),
            codex_base_url_env: self.codex_base_url_env.or(defaults.codex_base_url_env),
        }
    }
}

impl ToPartial for OpenaiConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            auth: partial_opt(&self.auth, defaults.auth),
            api_key_env: partial_opt(&self.api_key_env, defaults.api_key_env),
            base_url: partial_opt(&self.base_url, defaults.base_url),
            base_url_env: partial_opt(&self.base_url_env, defaults.base_url_env),
            codex_base_url: partial_opt(&self.codex_base_url, defaults.codex_base_url),
            codex_base_url_env: partial_opt(&self.codex_base_url_env, defaults.codex_base_url_env),
        }
    }
}

#[cfg(test)]
#[path = "openai_tests.rs"]
mod tests;
