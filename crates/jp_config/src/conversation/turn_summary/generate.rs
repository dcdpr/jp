//! Turn summary generation configuration.

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt, delta_opt_partial, delta_opt_partial_at, path},
    fill::{self, FillDefaults},
    model::{ModelConfig, PartialModelConfig},
    partial::{ToPartial, partial_opt, partial_opt_config},
};

/// Turn summary generation configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct TurnSummaryGenerateConfig {
    /// Whether to automatically generate turn summaries in the background.
    ///
    /// Defaults to `true`.
    /// When enabled, each `jp query` summarizes up to `batch_size` earlier
    /// completed turns that do not yet have a summary.
    #[setting(default = true)]
    pub auto: bool,

    /// Maximum number of unsummarized turns to process in a single query.
    ///
    /// Defaults to `5`.
    /// Set to `0` to disable background turn summarization.
    #[setting(default = 5)]
    pub batch_size: usize,

    /// Model configuration specific to turn summary generation.
    ///
    /// If unset, falls back to `conversation.title.generate.model`, then to
    /// `assistant.model.id` with default parameters.
    /// If `parameters.reasoning` is unset on the resolved model, reasoning
    /// defaults to `low` effort with `exclude = true`.
    #[setting(nested)]
    pub model: Option<ModelConfig>,
}

impl AssignKeyValue for PartialTurnSummaryGenerateConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "auto" => self.auto = kv.try_some_bool()?,
            "batch_size" => self.batch_size = kv.try_some_number_or_from_str()?,
            _ if kv.p("model") => self.model.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialTurnSummaryGenerateConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            auto: delta_opt(self.auto.as_ref(), next.auto),
            batch_size: delta_opt(self.batch_size.as_ref(), next.batch_size),
            model: delta_opt_partial(self.model.as_ref(), next.model),
        }
    }

    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        Self {
            auto: delta_opt(self.auto.as_ref(), next.auto),
            batch_size: delta_opt(self.batch_size.as_ref(), next.batch_size),
            model: delta_opt_partial_at(
                &path(prefix, "model"),
                self.model.as_ref(),
                next.model,
                unsets,
            ),
        }
    }
}

impl FillDefaults for PartialTurnSummaryGenerateConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            auto: self.auto.or(defaults.auto),
            batch_size: self.batch_size.or(defaults.batch_size),
            model: fill::fill_opt(self.model, defaults.model),
        }
    }
}

impl ToPartial for TurnSummaryGenerateConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            auto: partial_opt(&self.auto, None),
            batch_size: partial_opt(&self.batch_size, None),
            model: partial_opt_config(self.model.as_ref(), None),
        }
    }
}
