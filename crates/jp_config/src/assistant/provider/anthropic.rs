use confique::Config as Confique;
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    error::Result,
};

/// Anthropic API configuration.
#[derive(Debug, Clone, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Anthropic {
    /// Environment variable that contains the API key.
    #[config(
        default = "ANTHROPIC_API_KEY",
        env = "JP_ASSISTANT_PROVIDER_ANTHROPIC_API_KEY_ENV"
    )]
    pub api_key_env: String,

    /// The base URL to use for API requests.
    #[config(
        default = "https://api.anthropic.com",
        env = "JP_ASSISTANT_PROVIDER_ANTHROPIC_BASE_URL"
    )]
    pub base_url: String,
}

impl Default for Anthropic {
    fn default() -> Self {
        Self {
            api_key_env: "ANTHROPIC_API_KEY".to_owned(),
            base_url: "https://api.anthropic.com".to_owned(),
        }
    }
}

impl AssignKeyValue for <Anthropic as Confique>::Partial {
    fn assign(&mut self, kv: KvAssignment) -> Result<()> {
        match kv.key().as_str() {
            "api_key_env" => self.api_key_env = Some(kv.try_into_string()?),
            "base_url" => self.base_url = Some(kv.try_into_string()?),
            _ => return Err(set_error(kv.key())),
        }

        Ok(())
    }
}
