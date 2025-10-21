//! Deepgram TTS provider configurations.

use schematic::Config;

use crate::assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment};

/// Deepgram provider configuration.
#[derive(Debug, Clone, Config)]
#[config(rename_all = "snake_case")]
pub struct DeepgramConfig {
    /// TODO
    #[expect(dead_code)]
    todo: (),
}

impl AssignKeyValue for PartialDeepgramConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}
