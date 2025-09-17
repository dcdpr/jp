//! Tool call styling configuration.

use schematic::Config;

use crate::assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment};

/// Tool call content style configuration.
#[derive(Debug, Config)]
#[config(rename_all = "snake_case")]
pub struct ToolCallConfig {
    /// Whether to show the "tool call" text.
    ///
    /// Even if this is disabled, the model can still call tools and receive the
    /// results, but it will not be displayed.
    #[setting(default = true)]
    pub show: bool,
}

impl AssignKeyValue for PartialToolCallConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "show" => self.show = kv.try_some_bool()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

