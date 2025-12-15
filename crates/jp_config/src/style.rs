//! Style configuration for output formatting.

pub mod code;
pub mod reasoning;
pub mod tool_call;
pub mod typewriter;

use schematic::{Config, ConfigEnum};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::PartialConfigDelta,
    partial::ToPartial,
    style::{
        code::{CodeConfig, PartialCodeConfig},
        reasoning::{PartialReasoningConfig, ReasoningConfig},
        tool_call::{PartialToolCallConfig, ToolCallConfig},
        typewriter::{PartialTypewriterConfig, TypewriterConfig},
    },
};

/// Style configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct StyleConfig {
    /// Fenced code block style.
    #[setting(nested)]
    pub code: CodeConfig,

    /// Reasoning content style.
    #[setting(nested)]
    pub reasoning: ReasoningConfig,

    /// Tool call content style.
    #[setting(nested)]
    pub tool_call: ToolCallConfig,

    /// Typewriter style.
    #[setting(nested)]
    pub typewriter: TypewriterConfig,
}

impl AssignKeyValue for PartialStyleConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            _ if kv.p("code") => self.code.assign(kv)?,
            _ if kv.p("reasoning") => self.reasoning.assign(kv)?,
            _ if kv.p("tool_call") => self.tool_call.assign(kv)?,
            _ if kv.p("typewriter") => self.typewriter.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialStyleConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            code: self.code.delta(next.code),
            reasoning: self.reasoning.delta(next.reasoning),
            tool_call: self.tool_call.delta(next.tool_call),
            typewriter: self.typewriter.delta(next.typewriter),
        }
    }
}

impl ToPartial for StyleConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            code: self.code.to_partial(),
            reasoning: self.reasoning.to_partial(),
            tool_call: self.tool_call.to_partial(),
            typewriter: self.typewriter.to_partial(),
        }
    }
}

/// Formatting style for links.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "lowercase")]
pub enum LinkStyle {
    /// No link.
    Off,
    /// Unformatted link.
    Full,
    /// Link with OSC-8 escape sequences.
    #[default]
    Osc8,
}
