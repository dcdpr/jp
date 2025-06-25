use confique::Config as Confique;
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    error::Result,
};

/// Ollama API configuration.
#[derive(Debug, Clone, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Ollama {
    /// The base URL to use for API requests.
    #[config(default = "http://localhost:11434")]
    pub base_url: String,
}

impl Default for Ollama {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:11434".to_owned(),
        }
    }
}

impl AssignKeyValue for <Ollama as Confique>::Partial {
    fn assign(&mut self, kv: KvAssignment) -> Result<()> {
        match kv.key().as_str() {
            "base_url" => self.base_url = Some(kv.try_into_string()?),

            _ => return set_error(kv.key()),
        }

        Ok(())
    }
}
