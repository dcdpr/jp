//! Title configuration for conversations.

pub mod generate;

use schematic::Config;

use crate::{
    assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment},
    conversation::title::generate::{GenerateConfig, PartialGenerateConfig},
    delta::PartialConfigDelta,
    partial::ToPartial,
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

impl PartialConfigDelta for PartialTitleConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            generate: self.generate.delta(next.generate),
        }
    }
}

impl ToPartial for TitleConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            generate: self.generate.to_partial(),
        }
    }
}
