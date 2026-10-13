//! Reasoning content styling configuration.

use std::{fmt, num::ParseIntError};

use schematic::{Config, ConfigEnum, TransformResult};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt, delta_opt_partial, delta_opt_partial_at, path},
    fill::{FillDefaults, fill_opt},
    model::{ModelConfig, PartialModelConfig},
    partial::{ToPartial, partial_opt, partial_opt_config, partial_opts},
    style::window_rows::WindowRows,
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
    ///
    /// `full`, `summary`, and `<number>` print the reasoning into the
    /// transcript, unless `window.rows` is set: then it streams into a window
    /// at the bottom of the terminal that is erased once the assistant stops
    /// reasoning.
    /// The other modes show no reasoning, so `window` has no effect on them.
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

    /// Show reasoning in a window at the bottom of the terminal while the
    /// assistant reasons, and erase it once reasoning ends.
    ///
    /// Off until `rows` is set.
    #[setting(nested)]
    pub window: ReasoningWindowConfig,
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
            _ if kv.p("window") => self.window.assign(kv)?,
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
            window: self.window.delta(next.window),
        }
    }

    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        Self {
            display: delta_opt(self.display.as_ref(), next.display),
            summary_model: delta_opt_partial_at(
                &path(prefix, "summary_model"),
                self.summary_model.as_ref(),
                next.summary_model,
                unsets,
            ),
            background: delta_opt(self.background.as_ref(), next.background),
            extend_across_tool_calls: delta_opt(
                self.extend_across_tool_calls.as_ref(),
                next.extend_across_tool_calls,
            ),
            window: self.window.delta(next.window),
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
            window: self.window.fill_from(defaults.window),
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
            window: self.window.to_partial(),
        }
    }
}

/// A window at the bottom of the terminal that shows the assistant's reasoning
/// while it reasons.
///
/// The window holds the most recent rows of reasoning, with a status row below
/// them unless `timer` is off, and is erased once the assistant stops
/// reasoning.
/// The reasoning is still recorded in the conversation.
/// `jp conversation print --style full` shows it afterwards.
///
/// ```toml
/// [style.reasoning.window]
/// rows = 8
/// ```
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct ReasoningWindowConfig {
    /// Rows of reasoning the window shows.
    ///
    /// - `false` or `0`: no window; reasoning is shown as
    ///   `style.reasoning.display` says.
    ///   This is the default.
    /// - `true`: size the window from the terminal height (a tenth of it).
    /// - `N`: show exactly `N` rows, still bounded by the terminal height.
    ///
    /// The window shows what `style.reasoning.display` renders: all of the
    /// reasoning with `full`, the first N characters with a number, and the
    /// summary with `summary`.
    /// `hidden`, `static`, `progress`, and `timer` show no reasoning, so the
    /// window has no effect with them.
    ///
    /// Where the window cannot be shown, for example when stdout is redirected
    /// to a file, when logs are printed to stderr with `-v`, or on a terminal
    /// too short to fit a row, reasoning is shown as `style.reasoning.display`
    /// says, as if no window were set.
    #[setting(default = "off")]
    pub rows: WindowRows,

    /// Show a status row below the window with how long the assistant has been
    /// reasoning.
    ///
    /// Defaults to `true`, which shows `⏱ Reasoning… 4.1s`.
    /// Set to `false` to show the rows of reasoning alone.
    /// Has no effect without a window.
    #[setting(default = true)]
    pub timer: bool,
}

impl AssignKeyValue for PartialReasoningWindowConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "rows" => self.rows = kv.try_some_bool_number_or_from_str()?,
            "timer" => self.timer = kv.try_some_bool()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialReasoningWindowConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            rows: delta_opt(self.rows.as_ref(), next.rows),
            timer: delta_opt(self.timer.as_ref(), next.timer),
        }
    }
}

impl FillDefaults for PartialReasoningWindowConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            rows: self.rows.or(defaults.rows),
            timer: self.timer.or(defaults.timer),
        }
    }
}

impl ToPartial for ReasoningWindowConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            rows: partial_opt(&self.rows, defaults.rows),
            timer: partial_opt(&self.timer, defaults.timer),
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

#[cfg(test)]
#[path = "reasoning_tests.rs"]
mod tests;
