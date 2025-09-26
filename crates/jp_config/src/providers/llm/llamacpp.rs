//! Llamacpp API configuration.

use schematic::Config;

use crate::{
    assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment},
    delta::{delta_opt, PartialConfigDelta},
    partial::{partial_opt, ToPartial},
};

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

impl PartialConfigDelta for PartialLlamacppConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            base_url: delta_opt(self.base_url.as_ref(), next.base_url),
        }
    }
}

impl ToPartial for LlamacppConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            base_url: partial_opt(&self.base_url, defaults.base_url),
        }
    }
}
