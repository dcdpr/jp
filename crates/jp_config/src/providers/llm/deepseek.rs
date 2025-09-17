//! Deepseek API configuration.

use schematic::Config;

use crate::assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment};

/// Deepseek API configuration.
#[derive(Debug, Clone, Config)]
#[config(rename_all = "snake_case")]
pub struct DeepseekConfig {
    /// Environment variable that contains the API key.
    #[setting(default = "DEEPSEEK_API_KEY")]
    pub api_key_env: String,
}

impl AssignKeyValue for PartialDeepseekConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "api_key_env" => self.api_key_env = kv.try_some_string()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}
