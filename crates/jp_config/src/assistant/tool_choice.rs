//! Rules for choosing which tools to call.

use schematic::ConfigEnum;
use serde::{Deserialize, Serialize};

/// How to call tools.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    /// Call zero, one, or multiple tools, at the discretion of the LLM.
    #[default]
    Auto,

    /// Force the LLM not to call any tools, even if any are available.
    None,

    /// Force the LLM to call at least one tool.
    Required,

    /// Require calling the specified named tool.
    #[variant(fallback)]
    Function(String),
}

impl ToolChoice {
    /// Returns `true` if the choice is a "forced call", i.e. requires the LLM
    /// to call at least one tool.
    #[must_use]
    pub const fn is_forced_call(&self) -> bool {
        matches!(self, Self::Required | Self::Function(_))
    }

    /// Returns the name of the function to call, if applicable.
    #[must_use]
    pub fn function_name(&self) -> Option<&str> {
        match self {
            Self::Function(name) => Some(name),
            _ => None,
        }
    }
}
