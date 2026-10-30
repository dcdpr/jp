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
    /// Whether to generate turn summaries with the LLM.
    ///
    /// Defaults to `true`.
    /// When enabled, a `jp query` that finds earlier turns without a summary
    /// summarizes up to `batch_size` of them in one background request, which
    /// runs alongside the new turn.
    /// A turn is therefore summarized during the query after it, so the latest
    /// turn has no summary until the next `jp query`.
    #[setting(default = true)]
    pub auto: bool,

    /// Maximum number of turns summarized in one `jp query`.
    ///
    /// Defaults to `5`.
    /// A conversation with many turns lacking a summary, such as one started
    /// before turn summaries existed, catches up over several queries instead
    /// of in one large request.
    #[setting(default = 5)]
    pub batch_size: usize,

    /// Model used to summarize turns.
    ///
    /// If unset, falls back to `conversation.title.generate.model`, then to
    /// `assistant.model.id` with default parameters.
    /// A fast or local model suits this: the request runs in the background,
    /// but a slow one can delay `jp query` from exiting.
    /// Tool output and reasoning are left out of the request, so a small
    /// context window fits most turns; a turn that still does not fit is not
    /// summarized.
    ///
    /// If the model's `parameters.reasoning` is unset, reasoning runs at low
    /// effort with `exclude = true`.
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
