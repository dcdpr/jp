//! Llamacpp API configuration.

use schematic::Config;

use crate::assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment};

/// Llamacpp API configuration.
#[derive(Debug, Clone, Config)]
#[config(rename_all = "snake_case")]
pub struct LlamacppConfig {
    /// The base URL to use for API requests.
    #[setting(default = "http://127.0.0.1:8080")]
    pub base_url: String,
}

impl AssignKeyValue for PartialLlamacppConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "base_url" => self.base_url = kv.try_some_string()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}
