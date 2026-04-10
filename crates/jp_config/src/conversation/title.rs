//! Title configuration for conversations.

pub mod generate;

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    conversation::title::generate::{GenerateConfig, PartialGenerateConfig},
    delta::PartialConfigDelta,
    fill::FillDefaults,
    partial::ToPartial,
};

/// Title configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct TitleConfig {
    /// Title generation configuration.
    ///
    /// Configures how and when titles are automatically generated for new
    /// conversations.
    #[setting(nested)]
    pub generate: GenerateConfig,
}

impl AssignKeyValue for PartialTitleConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
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

impl FillDefaults for PartialTitleConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            generate: self.generate.fill_from(defaults.generate),
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
