//! Turn summary configuration for conversations.
//!
//! Controls automatic generation of single-line turn summaries stored on each
//! turn's `turn_start` entry in `events.json`.
//!
//! ```toml
//! [conversation.turn_summary.generate]
//! auto = true
//! batch_size = 5
//!
//! [conversation.turn_summary.generate.model]
//! id = "ollama/llama3.2:3b"
//! ```

pub mod generate;

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    conversation::turn_summary::generate::{
        PartialTurnSummaryGenerateConfig, TurnSummaryGenerateConfig,
    },
    delta::{PartialConfigDelta, path},
    fill::FillDefaults,
    partial::ToPartial,
};

/// Turn summary configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct TurnSummaryConfig {
    /// Turn summary generation settings.
    #[setting(nested)]
    pub generate: TurnSummaryGenerateConfig,
}

impl AssignKeyValue for PartialTurnSummaryConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("generate") => self.generate.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialTurnSummaryConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            generate: self.generate.delta(next.generate),
        }
    }

    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        Self {
            generate: self.generate.delta_with_unsets(
                next.generate,
                &path(prefix, "generate"),
                unsets,
            ),
        }
    }
}

impl FillDefaults for PartialTurnSummaryConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            generate: self.generate.fill_from(defaults.generate),
        }
    }
}

impl ToPartial for TurnSummaryConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            generate: self.generate.to_partial(),
        }
    }
}

#[cfg(test)]
#[path = "turn_summary_tests.rs"]
mod tests;
