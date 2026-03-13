//! LLM model configuration.

pub mod id;
pub mod parameters;

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::PartialConfigDelta,
    model::{
        id::{ModelIdOrAliasConfig, PartialModelIdOrAliasConfig},
        parameters::{ParametersConfig, PartialParametersConfig},
    },
    partial::ToPartial,
};

/// Assistant-specific configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case", allow_unknown_fields)]
pub struct ModelConfig {
    /// The model ID.
    ///
    /// This identifies the LLM model to use. It can be a full ID (e.g.
    /// `anthropic/claude-3-opus-20240229`) or an alias.
    #[setting(nested)]
    pub id: ModelIdOrAliasConfig,

    /// The model parameters.
    ///
    /// Configuration for model parameters such as temperature, max tokens, etc.
    #[setting(nested)]
    pub parameters: ParametersConfig,
}

impl AssignKeyValue for PartialModelConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("id") => self.id.assign(kv)?,
            _ if kv.p("parameters") => self.parameters.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialModelConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            id: self.id.delta(next.id),
            parameters: self.parameters.delta(next.parameters),
        }
    }
}

impl ToPartial for ModelConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            id: self.id.to_partial(),
            parameters: self.parameters.to_partial(),
        }
    }
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
