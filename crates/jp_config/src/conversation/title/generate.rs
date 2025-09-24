//! Title generation configuration.

use schematic::Config;

use crate::{
    assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment},
    model::{ModelConfig, PartialModelConfig},
};

/// Title generation configuration.
#[derive(Debug, Config)]
#[config(rename_all = "snake_case")]
pub struct GenerateConfig {
    /// Whether to automatically generate titles for conversations.
    #[setting(default = true)]
    pub auto: bool,

    /// Model configuration specific to title generation.
    #[setting(nested)]
    pub model: Option<ModelConfig>,
}

impl AssignKeyValue for PartialGenerateConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "auto" => self.auto = kv.try_some_bool()?,
            _ if kv.p("model") => self.model.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}
