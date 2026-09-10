//! Anthropic API configuration.

use schematic::{Config, ConfigError};

// Re-exported so `providers.llm.anthropic`'s own chain type is reachable
// alongside its config, though the grammar itself is shared.
pub use crate::providers::llm::{AuthEntry, AuthEntryParseError};
use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt, delta_opt_vec, delta_opt_vec_at, path},
    fill::FillDefaults,
    internal::merge::append_vec_dedup,
    partial::{ToPartial, partial_opt},
    types::api_key_env::ApiKeyEnv,
    validate::Validator,
};

/// The configuration path the credential chain lives at.
const AUTH_KEY: &str = "providers.llm.anthropic.auth";

/// Anthropic API configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct AnthropicConfig {
    /// The credentials a request may authenticate with, in fallback order.
    ///
    /// Defaults to `["api_key"]`.
    ///
    /// Each entry selects a credential source:
    ///
    /// - `api_key`: Metered billing, using the key `api_key_env` names.
    /// - `api_key:<name>`: Metered billing with the named key, when
    ///   `api_key_env` maps several.
    /// - `subscription`: A plan's allowance, using the sole stored credential.
    ///   Log in with `jp provider auth login llm.anthropic`.
    /// - `subscription:<name>`: The named stored credential.
    ///
    /// `api` and `sub` are accepted as shorthand for the two kinds.
    /// Names are case-sensitive.
    ///
    /// Entries are tried in order: when one cannot produce a usable credential,
    /// JP continues with the next.
    /// Listing `api_key` after a subscription profile authorizes continuing on
    /// per-token billing once the subscription allowance is gone.
    ///
    /// Replaces the chain from earlier config layers rather than extending it.
    ///
    /// ```toml
    /// [providers.llm.anthropic]
    /// auth = ["subscription:personal", "subscription:work", "api_key"]
    /// ```
    #[setting(default = vec![AuthEntry::ApiKey(None)])]
    pub auth: Vec<AuthEntry>,

    /// Environment variable that contains the API key.
    ///
    /// A map names several keys, each selectable from the `auth` chain as
    /// `api_key:<name>`:
    ///
    /// ```toml
    /// api_key_env = { work = "WORK_ANTHROPIC_KEY", personal = "MY_ANTHROPIC_KEY" }
    /// ```
    #[setting(default = "ANTHROPIC_API_KEY")]
    pub api_key_env: ApiKeyEnv,

    /// The base URL to use for API requests.
    #[setting(default = "https://api.anthropic.com")]
    pub base_url: String,

    /// Whether to chain multiple requests when a request is stopped early due
    /// to exceeding the maximum number of tokens allowed by the model.
    ///
    /// This is enabled by default, but even when enabled, if you explicitly set
    /// the model's `max_tokens` parameter, the request will not be chained when
    /// that limit is reached.
    /// This allows for better cost control.
    #[setting(default = true)]
    pub chain_on_max_tokens: bool,

    /// Any optional headers to enable beta features.
    ///
    /// See: <https://docs.anthropic.com/en/api/beta-headers>
    ///
    /// To find out which beta headers are available, see:
    /// <https://docs.anthropic.com/en/release-notes/api>
    #[setting(default = vec![], merge = append_vec_dedup)]
    pub beta_headers: Vec<String>,
}

impl Validator for AnthropicConfig {
    /// Rejects an `auth` chain that is empty or repeats an entry.
    ///
    /// An unrecognized entry is rejected earlier, when the string is parsed
    /// into an [`AuthEntry`].
    fn validate(&self) -> Result<(), ConfigError> {
        AuthEntry::validate_chain(&self.auth, AUTH_KEY)
    }
}

impl AssignKeyValue for PartialAnthropicConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "api_key_env" => self.api_key_env = kv.try_some_object_or_from_str()?,
            "base_url" => self.base_url = kv.try_some_string()?,
            "chain_on_max_tokens" => self.chain_on_max_tokens = kv.try_some_bool()?,
            _ if kv.p("auth") => {
                kv.try_some_vec(&mut self.auth, |kv| match kv.value.into_value() {
                    serde_json::Value::String(s) => s.parse().map_err(Into::into),
                    value => Err(format!("expected a string, got {value}").into()),
                })?;
            }
            _ if kv.p("beta_headers") => kv.try_some_vec_of_strings(&mut self.beta_headers)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialAnthropicConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            auth: delta_opt(self.auth.as_ref(), next.auth),
            api_key_env: delta_opt(self.api_key_env.as_ref(), next.api_key_env),
            base_url: delta_opt(self.base_url.as_ref(), next.base_url),
            chain_on_max_tokens: delta_opt(
                self.chain_on_max_tokens.as_ref(),
                next.chain_on_max_tokens,
            ),
            beta_headers: delta_opt_vec(self.beta_headers.as_ref(), next.beta_headers),
        }
    }

    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        Self {
            // `auth` merges by replacement, so merging reaches `next`
            // without clearing the field first.
            auth: delta_opt(self.auth.as_ref(), next.auth),
            api_key_env: delta_opt(self.api_key_env.as_ref(), next.api_key_env),
            base_url: delta_opt(self.base_url.as_ref(), next.base_url),
            chain_on_max_tokens: delta_opt(
                self.chain_on_max_tokens.as_ref(),
                next.chain_on_max_tokens,
            ),
            beta_headers: delta_opt_vec_at(
                &path(prefix, "beta_headers"),
                self.beta_headers.as_ref(),
                next.beta_headers,
                unsets,
            ),
        }
    }
}

impl FillDefaults for PartialAnthropicConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            auth: self.auth.or(defaults.auth),
            api_key_env: self.api_key_env.or(defaults.api_key_env),
            base_url: self.base_url.or(defaults.base_url),
            chain_on_max_tokens: self.chain_on_max_tokens.or(defaults.chain_on_max_tokens),
            beta_headers: self.beta_headers.or(defaults.beta_headers),
        }
    }
}

impl ToPartial for AnthropicConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            auth: partial_opt(&self.auth, defaults.auth),
            api_key_env: partial_opt(&self.api_key_env, defaults.api_key_env),
            base_url: partial_opt(&self.base_url, defaults.base_url),
            chain_on_max_tokens: partial_opt(
                &self.chain_on_max_tokens,
                defaults.chain_on_max_tokens,
            ),
            beta_headers: partial_opt(&self.beta_headers, defaults.beta_headers),
        }
    }
}

#[cfg(test)]
#[path = "anthropic_tests.rs"]
mod tests;
