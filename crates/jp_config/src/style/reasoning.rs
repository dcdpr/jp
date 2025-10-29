//! Reasoning content styling configuration.

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    partial::{ToPartial, partial_opt},
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

impl ToPartial for ReasoningConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            show: partial_opt(&self.show, defaults.show),
        }
    }
}
