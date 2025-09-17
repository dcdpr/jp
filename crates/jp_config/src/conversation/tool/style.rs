//! Display style configuration for tools.

use std::{fmt, num::ParseIntError};

use schematic::{Config, ConfigEnum};
use serde::{Deserialize, Serialize};

use crate::assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment};

/// Display style configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct DisplayStyleConfig {
    /// How to display the results of the tool call.
    #[setting(default)]
    pub inline_results: InlineResults,

    /// How to display the link to the file containing the tool call results.
    #[setting(default)]
    pub results_file_link: LinkStyle,
}

impl AssignKeyValue for PartialDisplayStyleConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "inline_results" => self.inline_results = kv.try_some_from_str()?,
            "results_file_link" => self.results_file_link = kv.try_some_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

/// Whether and how to show the tool call results inline in the terminal.
///
/// Even if disabled or truncated, a link will be added to a file containing the
/// full tool call results. Additionally, the full tool call results will be
/// sent back to the assistant, regardless of this setting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum InlineResults {
    /// Never show the tool call results inline.
    Off,

    /// Show the full tool call results inline.
    Full,

    /// Show the first N lines of the tool call results inline.
    #[variant(fallback)]
    Truncate(Truncate),
}

impl Default for InlineResults {
    fn default() -> Self {
        Self::Truncate(Truncate::default())
    }
}

/// Truncate the tool call results to the first N lines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Truncate {
    /// The number of lines to show.
    pub lines: usize,
}

impl Default for Truncate {
    fn default() -> Self {
        Self { lines: 10 }
    }
}

impl TryFrom<&str> for Truncate {
    type Error = ParseIntError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse().map(|lines| Self { lines })
    }
}

impl fmt::Display for Truncate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.lines)
    }
}

/// How to display the link to the file containing the tool call results.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "lowercase")]
pub enum LinkStyle {
    /// Full (raw) link.
    #[default]
    Full,

    /// Clickable link using the `osc8` escape sequence.
    Osc8,

    /// No link.
    Off,
}
