//! Anthropic API configuration.

use schematic::Config;

use crate::assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment};

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

    /// Any optional headers to enable beta features.
    ///
    /// See: <https://docs.anthropic.com/en/api/beta-headers>
    ///
    /// To find out which beta headers are available, see:
    /// <https://docs.anthropic.com/en/release-notes/api>
    #[setting(default = vec![], merge = schematic::merge::append_vec)]
    pub beta_headers: Vec<String>,
}

impl AssignKeyValue for PartialAnthropicConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "api_key_env" => self.api_key_env = kv.try_some_string()?,
            "base_url" => self.base_url = kv.try_some_string()?,
            "beta_headers" => kv.try_some_vec_of_strings(&mut self.beta_headers)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}
