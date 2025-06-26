use confique::Config as Confique;
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    error::Result,
};

/// Openai API configuration.
#[derive(Debug, Clone, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
pub struct Llamacpp {
    /// The base URL to use for API requests.
    #[config(default = "http://127.0.0.1:8080")]
    pub base_url: String,
}

impl Default for Llamacpp {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:8080".to_owned(),
        }
    }
}

impl AssignKeyValue for <Llamacpp as Confique>::Partial {
    fn assign(&mut self, kv: KvAssignment) -> Result<()> {
        match kv.key().as_str() {
            "base_url" => self.base_url = Some(kv.try_into_string()?),

            _ => return set_error(kv.key()),
        }

        Ok(())
    }
}
