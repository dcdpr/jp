//! Title configuration for conversations.

pub mod generate;

use schematic::Config;

use crate::{
    assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment},
    conversation::title::generate::{GenerateConfig, PartialGenerateConfig},
};

/// Title configuration.
#[derive(Debug, Config)]
#[config(rename_all = "snake_case")]
pub struct TitleConfig {
    /// Title generation configuration.
    #[setting(nested)]
    pub generate: GenerateConfig,
}

impl AssignKeyValue for PartialTitleConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            _ if kv.p("generate") => self.generate.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}
