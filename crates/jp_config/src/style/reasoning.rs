//! Reasoning content styling configuration.

use schematic::Config;

use crate::{
    assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment},
    delta::{delta_opt, PartialConfigDelta},
};

/// Reasoning content style configuration.
#[derive(Debug, Config)]
#[config(rename_all = "snake_case")]
pub struct ReasoningConfig {
    /// Whether to show reasoning blocks.
    #[setting(default = true)]
    pub show: bool,
}

impl AssignKeyValue for PartialReasoningConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "show" => self.show = kv.try_some_bool()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialReasoningConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            show: delta_opt(self.show.as_ref(), next.show),
        }
    }
}
