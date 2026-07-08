//! Reasoning content styling configuration.

use std::{fmt, num::ParseIntError};

use schematic::{Config, ConfigEnum, TransformResult};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt, delta_opt_partial},
    fill::{FillDefaults, fill_opt},
    model::{ModelConfig, PartialModelConfig},
    partial::{ToPartial, partial_opt, partial_opt_config, partial_opts},
    types::color::Color,
};

/// Reasoning content style configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct ReasoningConfig {
    /// How to display the reasoning content.
    ///
    /// - `full`: Show all reasoning content (default).
    /// - `hidden`: Do not show reasoning content.
    /// - `summary`: Show a summary of the reasoning (requires `summary_model`).
    /// - `static`: Show a static "reasoning..." message.
    /// - `progress`: Show "reasoning..." with animated dots.
    /// - `timer`: Show a running timer while reasoning.
    /// - `<number>`: Show the first N characters of the reasoning content.
    #[setting(default)]
    pub display: ReasoningDisplayConfig,

    /// The model to use for summarizing reasoning blocks.
    ///
    /// Defaults to the assistant's default model, but with reasoning disabled.
    ///
    /// Only used when `display` is set to `summary`.
    #[setting(nested)]
    pub summary_model: Option<ModelConfig>,

    /// Background color for reasoning content.
    ///
    /// When set, reasoning blocks are rendered with this background color
    /// spanning the full terminal width, visually distinguishing them from
    /// regular message content.
    ///
    /// Accepts either an ANSI 256-color index (e.g. `236`) or a hex RGB string
    /// (e.g. `"#1d2021"`).
    #[setting(default = default_reasoning_background)]
    pub background: Option<Color>,

    /// Extend the reasoning background across tool calls made while reasoning.
    ///
    /// Defaults to `true`.
    /// When the assistant interleaves reasoning with tool calls, each tool
    /// call's chrome (the `Calling tool …` header, arguments, progress, and
    /// result) is shaded with `background` so the reasoning region reads as one
    /// continuous span, instead of the background switching off for every tool
    /// call and back on for the next reasoning block.
    ///
    /// Has no effect when `background` is unset.
    /// Set to `false` to keep each reasoning block and tool call on its own
    /// background.
    #[setting(default = true)]
    pub extend_across_tool_calls: bool,
}

/// The default reasoning background color.
#[expect(clippy::trivially_copy_pass_by_ref, clippy::unnecessary_wraps)]
const fn default_reasoning_background(_: &()) -> TransformResult<Option<Color>> {
    Ok(Some(Color::Ansi256(236)))
}

impl AssignKeyValue for PartialReasoningConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "display" => self.display = kv.try_some_from_str()?,
            "background" => self.background = kv.try_some_number_or_from_str()?,
            "extend_across_tool_calls" => {
                self.extend_across_tool_calls = kv.try_some_bool()?;
            }
            _ if kv.p("summary_model") => self.summary_model.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialReasoningConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            display: delta_opt(self.display.as_ref(), next.display),
            summary_model: delta_opt_partial(self.summary_model.as_ref(), next.summary_model),
            background: delta_opt(self.background.as_ref(), next.background),
            extend_across_tool_calls: delta_opt(
                self.extend_across_tool_calls.as_ref(),
                next.extend_across_tool_calls,
            ),
        }
    }
}

impl FillDefaults for PartialReasoningConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            display: self.display.or(defaults.display),
            summary_model: fill_opt(self.summary_model, defaults.summary_model),
            background: self.background.or(defaults.background),
            extend_across_tool_calls: self
                .extend_across_tool_calls
                .or(defaults.extend_across_tool_calls),
        }
    }
}

impl ToPartial for ReasoningConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            display: partial_opt(&self.display, defaults.display),
            summary_model: partial_opt_config(self.summary_model.as_ref(), defaults.summary_model),
            background: partial_opts(self.background.as_ref(), defaults.background),
            extend_across_tool_calls: partial_opt(
                &self.extend_across_tool_calls,
                defaults.extend_across_tool_calls,
            ),
        }
    }
}

/// How to display the reasoning content.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ReasoningDisplayConfig {
    /// Reasoning is hidden, regardless of the model's capabilities.
    Hidden,

    /// A summary of the reasoning is displayed.
    /// This summary is generated by a separate model.
    Summary,

    /// A static "reasoning..." message is displayed while the assistant is
    /// reasoning.
    Static,

    /// Similar to `Static`, but additional dots are added to indicate that the
    /// assistant is still reasoning.
    Progress,

    /// Show a running timer while the assistant is reasoning.
    /// The timer is erased when reasoning completes and message content begins
    /// streaming.
    Timer,

    /// Reasoning content is displayed as it is generated.
    #[default]
    Full,

    /// Show the first N characters of the reasoning content.
    #[variant(fallback)]
    Truncate(TruncateChars),
}

/// Truncate the tool call results to the first N lines.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TruncateChars {
    /// The number of characters to show.
    pub characters: usize,
}

impl Default for TruncateChars {
    fn default() -> Self {
        Self { characters: 100 }
    }
}

impl TryFrom<&str> for TruncateChars {
    type Error = ParseIntError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse().map(|characters| Self { characters })
    }
}

impl fmt::Display for TruncateChars {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.characters)
    }
}
