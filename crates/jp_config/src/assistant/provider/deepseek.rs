use confique::Config as Confique;
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    error::Result,
};

/// Deepseek API configuration.
#[derive(Debug, Clone, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Deepseek {
    /// Environment variable that contains the API key.
    #[config(default = "DEEPSEEK_API_KEY")]
    pub api_key_env: String,
}

impl Default for Deepseek {
    fn default() -> Self {
        Self {
            api_key_env: "DEEPSEEK_API_KEY".to_owned(),
        }
    }
}

impl AssignKeyValue for <Deepseek as Confique>::Partial {
    fn assign(&mut self, kv: KvAssignment) -> Result<()> {
        match kv.key().as_str() {
            "api_key_env" => self.api_key_env = Some(kv.try_into_string()?),
            _ => return set_error(kv.key()),
        }

        Ok(())
    }
}
