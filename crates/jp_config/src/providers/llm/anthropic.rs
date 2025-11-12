//! Anthropic API configuration.

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt, delta_opt_vec},
    partial::{ToPartial, partial_opt},
    util,
};

/// Anthropic API configuration.
#[derive(Debug, Clone, Config)]
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
    /// that limit is reached. This allows for better cost control.
    ///
    /// Further note that if/when an API response contains a tool call request,
    /// no chaining will be performed, as the API expects the next request to
    /// contain the tool call results.
    ///
    /// Finally, be aware that there is a performance penalty for enabling this
    /// feature, as the client has to copy the received messages in order to
    /// append them to the next request.
    #[setting(default = true)]
    pub chain_on_max_tokens: bool,

    /// Any optional headers to enable beta features.
    ///
    /// See: <https://docs.anthropic.com/en/api/beta-headers>
    ///
    /// To find out which beta headers are available, see:
    /// <https://docs.anthropic.com/en/release-notes/api>
    #[setting(default = vec![], merge = schematic::merge::append_vec, transform = util::vec_dedup)]
    pub beta_headers: Vec<String>,
}

impl AssignKeyValue for PartialAnthropicConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "api_key_env" => self.api_key_env = kv.try_some_string()?,
            "base_url" => self.base_url = kv.try_some_string()?,
            "chain_on_max_tokens" => self.chain_on_max_tokens = kv.try_some_bool()?,
            "beta_headers" => kv.try_some_vec_of_strings(&mut self.beta_headers)?,
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
        }
    }
}
