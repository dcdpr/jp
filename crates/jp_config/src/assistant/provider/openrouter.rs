use confique::Config as Confique;
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    error::Result,
};

/// Openrouter API configuration.
#[derive(Debug, Clone, Confique, PartialEq, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Openrouter {
    /// Environment variable that contains the API key.
    #[config(default = "OPENROUTER_API_KEY")]
    pub api_key_env: String,

    /// Application name sent to Openrouter.
    #[config(default = "JP")]
    pub app_name: String,

    /// Optional HTTP referrer to send with requests.
    pub app_referrer: Option<String>,

    /// The base URL to use for API requests.
    #[config(default = "https://openrouter.ai")]
    pub base_url: String,
}

impl Default for Openrouter {
    fn default() -> Self {
        Self {
            api_key_env: "OPENROUTER_API_KEY".to_owned(),
            app_name: "JP".to_owned(),
            app_referrer: None,
            base_url: "https://openrouter.ai".to_owned(),
        }
    }
}

impl AssignKeyValue for <Openrouter as Confique>::Partial {
    fn assign(&mut self, kv: KvAssignment) -> Result<()> {
        match kv.key().as_str() {
            "api_key_env" => self.api_key_env = Some(kv.try_into_string()?),
            "app_name" => self.app_name = Some(kv.try_into_string()?),
            "app_referrer" => {
                self.app_referrer = kv.try_into_string().map(|v| (!v.is_empty()).then_some(v))?;
            }
            "base_url" => self.base_url = Some(kv.try_into_string()?),

            _ => return Err(set_error(kv.key())),
        }

        Ok(())
    }
}
