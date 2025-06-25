use confique::Config as Confique;
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    error::Result,
};

/// Openai API configuration.
#[derive(Debug, Clone, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Openai {
    /// Environment variable that contains the API key.
    #[config(default = "OPENAI_API_KEY")]
    pub api_key_env: String,

    /// The base URL to use for API requests.
    ///
    /// Used if `OPENAI_BASE_URL` is not set.
    #[config(default = "https://api.openai.com")]
    pub base_url: String,

    /// Environment variable that contains the API base URL key.
    #[config(default = "OPENAI_BASE_URL")]
    pub base_url_env: String,
}

impl Default for Openai {
    fn default() -> Self {
        Self {
            api_key_env: "OPENAI_API_KEY".to_owned(),
            base_url: "https://api.openai.com".to_owned(),
            base_url_env: "OPENAI_BASE_URL".to_owned(),
        }
    }
}

impl AssignKeyValue for <Openai as Confique>::Partial {
    fn assign(&mut self, kv: KvAssignment) -> Result<()> {
        match kv.key().as_str() {
            "api_key_env" => self.api_key_env = Some(kv.try_into_string()?),
            "base_url" => self.base_url = Some(kv.try_into_string()?),
            "base_url_env" => self.base_url_env = Some(kv.try_into_string()?),

            _ => return set_error(kv.key()),
        }

        Ok(())
    }
}
