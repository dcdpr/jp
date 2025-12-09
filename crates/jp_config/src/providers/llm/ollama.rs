//! Ollama API configuration.

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    partial::{ToPartial, partial_opt},
};

/// Ollama API configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct OllamaConfig {
    /// The base URL to use for API requests.
    #[setting(default = "http://localhost:11434")]
    pub base_url: String,
}

impl AssignKeyValue for PartialOllamaConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "base_url" => self.base_url = kv.try_some_string()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialOllamaConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            base_url: delta_opt(self.base_url.as_ref(), next.base_url),
        }
    }
}

impl ToPartial for OllamaConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            base_url: partial_opt(&self.base_url, defaults.base_url),
        }
    }
}
