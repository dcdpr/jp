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
