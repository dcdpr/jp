use confique::Config as Confique;
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    error::Result,
};

/// Google API configuration.
#[derive(Debug, Clone, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Google {
    /// Environment variable that contains the API key.
    #[config(default = "GEMINI_API_KEY")]
    pub api_key_env: String,

    /// The base URL to use for API requests.
    #[config(default = "https://generativelanguage.googleapis.com/v1beta")]
    pub base_url: String,
}

impl Default for Google {
    fn default() -> Self {
        Self {
            api_key_env: "GEMINI_API_KEY".to_owned(),
            base_url: "https://generativelanguage.googleapis.com/v1beta".to_owned(),
        }
    }
}

impl AssignKeyValue for <Google as Confique>::Partial {
    fn assign(&mut self, kv: KvAssignment) -> Result<()> {
        match kv.key().as_str() {
            "api_key_env" => self.api_key_env = Some(kv.try_into_string()?),
            "base_url" => self.base_url = Some(kv.try_into_string()?),
            _ => return Err(set_error(kv.key())),
        }

        Ok(())
    }
}
