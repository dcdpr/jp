//! Compaction configuration for conversations.

use std::{fmt, str::FromStr};

use schematic::{Config, ConfigEnum};
use serde::{Deserialize, Serialize};

use crate::{
    BoxedError,
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt, delta_opt_partial},
    fill::{self, FillDefaults},
    internal::merge::vec_with_strategy,
    model::{ModelConfig, PartialModelConfig},
    partial::{ToPartial, partial_opt_config, partial_opts},
    types::vec::{MergeableVec, MergedVec, vec_to_mergeable_partial},
};

/// Compaction configuration.
///
/// The `rules` array defines the compaction operations applied when the user
/// runs `jp conversation compact` or uses `--compact`. Each rule produces one
/// compaction event in the conversation stream.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct CompactionConfig {
    /// Compaction rules applied in order. Each rule produces one compaction
    /// event.
    ///
    /// The built-in default (strip reasoning + tools, keep last 3) is used
    /// when no rules are configured. It is discarded as soon as any
    /// user-defined rule is present.
    #[setting(
        nested,
        partial_via = MergeableVec::<CompactionRuleConfig>,
        default = default_rules,
        merge = vec_with_strategy,
    )]
    pub rules: Vec<CompactionRuleConfig>,
}

/// Built-in default rules: strip reasoning + tool calls, keep first 1, last 3.
///
/// Uses `discard_when_merged: true` so these defaults are dropped the moment
/// any user-defined rule appears.
#[expect(clippy::trivially_copy_pass_by_ref, clippy::unnecessary_wraps)]
fn default_rules(_: &()) -> schematic::TransformResult<MergeableVec<PartialCompactionRuleConfig>> {
    Ok(MergeableVec::Merged(MergedVec {
        value: vec![PartialCompactionRuleConfig {
            reasoning: Some(ReasoningMode::Strip),
            tool_calls: Some(ToolCallsMode::Strip),
            ..Default::default()
        }],
        strategy: None,
        dedup: None,
        discard_when_merged: true,
    }))
}

impl AssignKeyValue for PartialCompactionConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("rules") => kv.try_vec_of_nested(self.rules.as_mut())?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialCompactionConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            rules: {
                next.rules
                    .into_iter()
                    .filter(|v| !self.rules.contains(v))
                    .collect::<Vec<_>>()
                    .into()
            },
        }
    }
}

impl FillDefaults for PartialCompactionConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            rules: self.rules.fill_from(defaults.rules),
        }
    }
}

impl ToPartial for CompactionConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            rules: vec_to_mergeable_partial(&self.rules),
        }
    }
}

/// A compaction rule defining which policies to apply over a turn range.
///
/// Each rule produces one [`Compaction`] event when applied.
///
/// [`Compaction`]: https://docs.rs/jp_conversation/latest/jp_conversation/struct.Compaction.html
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct CompactionRuleConfig {
    /// Number of turns to preserve at the start of the conversation.
    ///
    /// Accepts a positive integer (turn count) or a duration string
    /// (e.g. `"5h"` — preserve turns from the last 5 hours).
    ///
    /// Defaults to 1 (preserve the initial request).
    #[setting(default = default_keep_first)]
    pub keep_first: RuleBound,

    /// Number of turns to preserve at the end of the conversation.
    ///
    /// Accepts a positive integer (turn count) or a duration string
    /// (e.g. `"3h"` — preserve turns from the last 3 hours).
    ///
    /// Defaults to 3 (keep last 3 turns).
    #[setting(default = default_keep_last)]
    pub keep_last: RuleBound,

    /// Policy for reasoning (thinking) blocks.
    pub reasoning: Option<ReasoningMode>,

    /// Policy for tool call arguments and responses.
    pub tool_calls: Option<ToolCallsMode>,

    /// Summarization configuration.
    ///
    /// When set, all events in the compacted range are replaced by a single
    /// LLM-generated summary. This takes precedence over `reasoning` and
    /// `tool_calls`.
    #[setting(nested)]
    pub summary: Option<SummaryConfig>,
}

/// Default `keep_first`: preserve the genesis turn.
#[expect(clippy::trivially_copy_pass_by_ref, clippy::unnecessary_wraps)]
const fn default_keep_first(_: &()) -> schematic::TransformResult<Option<RuleBound>> {
    Ok(Some(RuleBound::Turns(1)))
}

/// Default `keep_last`: preserve the last 3 turns.
#[expect(clippy::trivially_copy_pass_by_ref, clippy::unnecessary_wraps)]
const fn default_keep_last(_: &()) -> schematic::TransformResult<Option<RuleBound>> {
    Ok(Some(RuleBound::Turns(3)))
}

impl FromStr for PartialCompactionRuleConfig {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s).map_err(|e| format!("invalid compaction rule: {e}").into())
    }
}

impl AssignKeyValue for PartialCompactionRuleConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "keep_first" => self.keep_first = kv.try_some_from_str()?,
            "keep_last" => self.keep_last = kv.try_some_from_str()?,
            "reasoning" => self.reasoning = kv.try_some_from_str()?,
            "tool_calls" => self.tool_calls = kv.try_some_from_str()?,
            _ if kv.p("summary") => self.summary.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialCompactionRuleConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            keep_first: delta_opt(self.keep_first.as_ref(), next.keep_first),
            keep_last: delta_opt(self.keep_last.as_ref(), next.keep_last),
            reasoning: delta_opt(self.reasoning.as_ref(), next.reasoning),
            tool_calls: delta_opt(self.tool_calls.as_ref(), next.tool_calls),
            summary: delta_opt_partial(self.summary.as_ref(), next.summary),
        }
    }
}

impl FillDefaults for PartialCompactionRuleConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            keep_first: self.keep_first.or(defaults.keep_first),
            keep_last: self.keep_last.or(defaults.keep_last),
            reasoning: self.reasoning.or(defaults.reasoning),
            tool_calls: self.tool_calls.or(defaults.tool_calls),
            summary: fill::fill_opt(self.summary, defaults.summary),
        }
    }
}

impl ToPartial for CompactionRuleConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            keep_first: partial_opts(Some(&self.keep_first), None),
            keep_last: partial_opts(Some(&self.keep_last), None),
            reasoning: partial_opts(self.reasoning.as_ref(), None),
            tool_calls: partial_opts(self.tool_calls.as_ref(), None),
            summary: self.summary.as_ref().map(ToPartial::to_partial),
        }
    }
}

/// Summarization configuration for a compaction rule.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct SummaryConfig {
    /// Model to use for summarization.
    ///
    /// If not set, the main assistant model is used.
    #[setting(nested)]
    pub model: Option<ModelConfig>,

    /// Custom instructions for the summarizer.
    ///
    /// If not set, a default prompt is used that preserves key decisions,
    /// file paths, error resolutions, and current task state.
    pub instructions: Option<String>,
}

impl AssignKeyValue for PartialSummaryConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("model") => self.model.assign(kv)?,
            "instructions" => self.instructions = kv.try_some_string()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialSummaryConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            model: delta_opt_partial(self.model.as_ref(), next.model),
            instructions: delta_opt(self.instructions.as_ref(), next.instructions),
        }
    }
}

impl FillDefaults for PartialSummaryConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            model: fill::fill_opt(self.model, defaults.model),
            instructions: self.instructions.or(defaults.instructions),
        }
    }
}

impl ToPartial for SummaryConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            model: partial_opt_config(self.model.as_ref(), None),
            instructions: partial_opts(self.instructions.as_ref(), None),
        }
    }
}

/// A range bound for compaction rules.
///
/// Rules only accept relative bounds (stable across invocations).
/// CLI flags extend this with absolute turn indices and dates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleBound {
    /// A number of turns to preserve.
    Turns(usize),
    /// Preserve turns within this duration, e.g. `"5h"`, `"2days"`.
    Duration(std::time::Duration),
    /// Start after the most recent compaction's `to_turn`.
    /// Only meaningful for `keep_first` (used via `from = "last"`).
    AfterLastCompaction,
}

impl FromStr for RuleBound {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("last") {
            return Ok(Self::AfterLastCompaction);
        }

        if let Ok(n) = s.parse::<usize>() {
            return Ok(Self::Turns(n));
        }

        humantime::parse_duration(s)
            .map(Self::Duration)
            .map_err(|e| format!("invalid range bound `{s}`: {e}").into())
    }
}

impl fmt::Display for RuleBound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Turns(n) => write!(f, "{n}"),
            Self::Duration(d) => write!(f, "{}", humantime::format_duration(*d)),
            Self::AfterLastCompaction => write!(f, "last"),
        }
    }
}

impl Serialize for RuleBound {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for RuleBound {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl RuleBound {
    /// A bound of zero turns.
    pub const ZERO: Self = Self::Turns(0);
}

impl Default for RuleBound {
    fn default() -> Self {
        Self::ZERO
    }
}

impl schematic::Schematic for RuleBound {
    fn build_schema(mut schema: schematic::SchemaBuilder) -> schematic::Schema {
        schema.string_default()
    }
}

/// How to handle reasoning blocks during compaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningMode {
    /// Strip all reasoning blocks from the projected view.
    Strip,
}

/// How to handle tool calls during compaction.
///
/// Parses from strings for config ergonomics:
/// - `"strip"` → strip both request arguments and response content
/// - `"strip-responses"` → strip response content only
/// - `"strip-requests"` → strip request arguments only
/// - `"omit"` → remove tool call pairs entirely
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallsMode {
    /// Strip both request arguments and response content.
    Strip,
    /// Strip response content only, keep request arguments.
    StripResponses,
    /// Strip request arguments only, keep response content.
    StripRequests,
    /// Remove tool call pairs entirely.
    Omit,
}

impl FromStr for ToolCallsMode {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "strip" => Ok(Self::Strip),
            "strip-responses" | "strip_responses" => Ok(Self::StripResponses),
            "strip-requests" | "strip_requests" => Ok(Self::StripRequests),
            "omit" => Ok(Self::Omit),
            _ => Err(format!("unknown tool_calls mode: `{s}`").into()),
        }
    }
}

impl fmt::Display for ToolCallsMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Strip => write!(f, "strip"),
            Self::StripResponses => write!(f, "strip-responses"),
            Self::StripRequests => write!(f, "strip-requests"),
            Self::Omit => write!(f, "omit"),
        }
    }
}

impl serde::Serialize for ToolCallsMode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for ToolCallsMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl schematic::Schematic for ToolCallsMode {
    fn build_schema(mut schema: schematic::SchemaBuilder) -> schematic::Schema {
        schema.string_default()
    }
}

#[cfg(test)]
#[path = "compaction_tests.rs"]
mod tests;
