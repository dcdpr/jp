use confique::Config as Confique;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

    /// Any optional headers to enable beta features.
    ///
    /// See: <https://docs.anthropic.com/en/api/beta-headers>
    ///
    /// To find out which beta headers are available, see:
    /// <https://docs.anthropic.com/en/release-notes/api>
    #[config(default = [], env = "JP_ASSISTANT_PROVIDER_ANTHROPIC_BETA_HEADERS")]
    pub beta_headers: Vec<String>,
}

impl Default for Anthropic {
    fn default() -> Self {
        Self {
            api_key_env: "ANTHROPIC_API_KEY".to_owned(),
            base_url: "https://api.anthropic.com".to_owned(),
            beta_headers: vec![],
        }
    }
}

impl AssignKeyValue for <Anthropic as Confique>::Partial {
    fn assign(&mut self, kv: KvAssignment) -> Result<()> {
        match kv.key().as_str() {
            "api_key_env" => self.api_key_env = Some(kv.try_into_string()?),
            "base_url" => self.base_url = Some(kv.try_into_string()?),
            "beta_headers" => {
                kv.try_set_or_merge_vec(self.beta_headers.get_or_insert_default(), |v| match v {
                    Value::String(v) => Ok(v),
                    _ => Err("Expected string".into()),
                })?;
            }
            _ => return Err(set_error(kv.key())),
        }

        Ok(())
    }
}
