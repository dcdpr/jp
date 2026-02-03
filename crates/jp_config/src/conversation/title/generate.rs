//! Title generation configuration.

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt, delta_opt_partial},
    model::{ModelConfig, PartialModelConfig},
    partial::{ToPartial, partial_opt, partial_opt_config},
};

/// Title generation configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct GenerateConfig {
    /// Whether to automatically generate titles for conversations.
    ///
    /// If true, a title will be generated based on the first prompt of the
    /// conversation.
    #[setting(default = true)]
    pub auto: bool,

    /// Model configuration specific to title generation.
    ///
    /// By default, the main assistant model is used. You can override this to
    /// use a faster or cheaper model specifically for title generation.
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

impl PartialConfigDelta for PartialGenerateConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            auto: delta_opt(self.auto.as_ref(), next.auto),
            model: delta_opt_partial(self.model.as_ref(), next.model),
        }
    }
}

impl ToPartial for GenerateConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            auto: partial_opt(&self.auto, None),
            model: partial_opt_config(self.model.as_ref(), None),
        }
    }
}
