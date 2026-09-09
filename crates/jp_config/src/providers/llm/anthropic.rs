//! Anthropic API configuration.

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt, delta_opt_vec, delta_opt_vec_at, path},
    fill::FillDefaults,
    internal::merge::append_vec_dedup,
    partial::{ToPartial, partial_opt},
};

/// Anthropic API configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct AnthropicConfig {
    /// Environment variable that contains the API key.
    #[setting(default = "ANTHROPIC_API_KEY")]
    pub api_key_env: String,

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

    /// How often to check whether a batched request has finished, in seconds.
    ///
    /// Defaults to `15`.
    ///
    /// Only used when `assistant.model.parameters.service_tier` is `flex`,
    /// which Anthropic serves through its Message Batches API.
    #[setting(default = 15)]
    pub batch_poll_interval_secs: u32,

    /// How long to wait for a batched request to finish, in seconds.
    ///
    /// Defaults to `3600` (one hour).
    /// Set to `0` to wait as long as Anthropic keeps the batch alive, which is
    /// 24 hours.
    ///
    /// Only used when `assistant.model.parameters.service_tier` is `flex`.
    /// Most batches finish within an hour.
    /// Giving up leaves the batch running and reports its id, so the answer can
    /// still be fetched from the Anthropic API by hand.
    #[setting(default = 3600)]
    pub batch_max_wait_secs: u32,
}

impl AssignKeyValue for PartialAnthropicConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "api_key_env" => self.api_key_env = kv.try_some_string()?,
            "base_url" => self.base_url = kv.try_some_string()?,
            "chain_on_max_tokens" => self.chain_on_max_tokens = kv.try_some_bool()?,
            "beta_headers" => kv.try_some_vec_of_strings(&mut self.beta_headers)?,
            "batch_poll_interval_secs" => {
                self.batch_poll_interval_secs = kv.try_some_u32()?;
            }
            "batch_max_wait_secs" => self.batch_max_wait_secs = kv.try_some_u32()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialAnthropicConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            api_key_env: delta_opt(self.api_key_env.as_ref(), next.api_key_env),
            base_url: delta_opt(self.base_url.as_ref(), next.base_url),
            chain_on_max_tokens: delta_opt(
                self.chain_on_max_tokens.as_ref(),
                next.chain_on_max_tokens,
            ),
            beta_headers: delta_opt_vec(self.beta_headers.as_ref(), next.beta_headers),
            batch_poll_interval_secs: delta_opt(
                self.batch_poll_interval_secs.as_ref(),
                next.batch_poll_interval_secs,
            ),
            batch_max_wait_secs: delta_opt(
                self.batch_max_wait_secs.as_ref(),
                next.batch_max_wait_secs,
            ),
        }
    }

    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        Self {
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
            batch_poll_interval_secs: delta_opt(
                self.batch_poll_interval_secs.as_ref(),
                next.batch_poll_interval_secs,
            ),
            batch_max_wait_secs: delta_opt(
                self.batch_max_wait_secs.as_ref(),
                next.batch_max_wait_secs,
            ),
        }
    }
}

impl FillDefaults for PartialAnthropicConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            api_key_env: self.api_key_env.or(defaults.api_key_env),
            base_url: self.base_url.or(defaults.base_url),
            chain_on_max_tokens: self.chain_on_max_tokens.or(defaults.chain_on_max_tokens),
            beta_headers: self.beta_headers.or(defaults.beta_headers),
            batch_poll_interval_secs: self
                .batch_poll_interval_secs
                .or(defaults.batch_poll_interval_secs),
            batch_max_wait_secs: self.batch_max_wait_secs.or(defaults.batch_max_wait_secs),
        }
    }
}

impl ToPartial for AnthropicConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            api_key_env: partial_opt(&self.api_key_env, defaults.api_key_env),
            base_url: partial_opt(&self.base_url, defaults.base_url),
            chain_on_max_tokens: partial_opt(
                &self.chain_on_max_tokens,
                defaults.chain_on_max_tokens,
            ),
            beta_headers: partial_opt(&self.beta_headers, defaults.beta_headers),
            batch_poll_interval_secs: partial_opt(
                &self.batch_poll_interval_secs,
                defaults.batch_poll_interval_secs,
            ),
            batch_max_wait_secs: partial_opt(
                &self.batch_max_wait_secs,
                defaults.batch_max_wait_secs,
            ),
        }
    }
}
