//! Conversation compaction types.
//!
//! Compaction is a non-destructive, additive operation that appends overlay
//! events to the conversation stream. These overlays instruct the projection
//! layer to present a reduced view when building the LLM request. The original
//! events are always preserved.
//!
//! See [RFD 064].
//!
//! [RFD 064]: https://github.com/dcdpr/jp/blob/main/docs/rfd/064-non-destructive-conversation-compaction.md

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A compaction overlay stored in the event stream.
///
/// Defines how events within `[from_turn, to_turn]` should be projected
/// when building the LLM request. The original events are unmodified.
///
/// Policies are optional: `None` means "this compaction has no opinion on this
/// content type" — the original events pass through, or an earlier compaction's
/// policy applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compaction {
    /// The timestamp when this compaction was created.
    #[serde(
        serialize_with = "crate::serialize_dt",
        deserialize_with = "crate::deserialize_dt"
    )]
    pub timestamp: DateTime<Utc>,

    /// First turn in the compacted range (inclusive, 0-based).
    pub from_turn: usize,

    /// Last turn in the compacted range (inclusive, 0-based).
    pub to_turn: usize,

    /// When set, replaces ALL provider-visible events in the range with a
    /// pre-computed summary. Takes precedence over `reasoning` and
    /// `tool_calls`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<SummaryPolicy>,

    /// Policy for `ChatResponse::Reasoning` events.
    /// Ignored when `summary` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningPolicy>,

    /// Policy for `ToolCallRequest` and `ToolCallResponse` pairs.
    /// Ignored when `summary` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<ToolCallPolicy>,
}

impl Compaction {
    /// Create a new compaction event for the given turn range.
    ///
    /// Timestamp is set to the current time. All policies default to `None`
    /// (pass-through).
    #[must_use]
    pub fn new(from_turn: usize, to_turn: usize) -> Self {
        Self {
            timestamp: Utc::now(),
            from_turn,
            to_turn,
            summary: None,
            reasoning: None,
            tool_calls: None,
        }
    }

    /// Set the reasoning policy.
    #[must_use]
    pub const fn with_reasoning(mut self, policy: ReasoningPolicy) -> Self {
        self.reasoning = Some(policy);
        self
    }

    /// Set the tool call policy.
    #[must_use]
    pub const fn with_tool_calls(mut self, policy: ToolCallPolicy) -> Self {
        self.tool_calls = Some(policy);
        self
    }

    /// Set the summary policy.
    #[must_use]
    pub fn with_summary(mut self, policy: SummaryPolicy) -> Self {
        self.summary = Some(policy);
        self
    }
}

/// Policy for handling reasoning events during compaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningPolicy {
    /// Omit all reasoning events from the projected view.
    Strip,
}

/// Replaces all provider-visible events in the compacted range with a
/// pre-computed summary.
///
/// Messages, reasoning, and tool calls are all replaced by a single synthetic
/// `ChatRequest`/`ChatResponse` pair containing the summary text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryPolicy {
    /// The summary text, generated at compaction-creation time.
    pub summary: String,
}

/// Policy for handling tool call request/response pairs during compaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case")]
pub enum ToolCallPolicy {
    /// Replace request arguments and/or response content with compact
    /// summaries. Preserves tool name, call ID, and success/error status.
    Strip {
        /// Replace arguments with a compact summary.
        request: bool,
        /// Replace response content with a status line.
        response: bool,
    },

    /// Remove all tool call pairs entirely.
    Omit,
}

/// A user-specified compaction range bound.
///
/// Bounds are resolved against a [`ConversationStream`] to produce absolute
/// turn indices. See [`self::resolve_range`].
///
/// [`ConversationStream`]: crate::ConversationStream
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeBound {
    /// Absolute 0-based turn index.
    Absolute(usize),
    /// Offset from the end. `FromEnd(3)` means 3 turns before the last.
    FromEnd(usize),
    /// The turn after the most recent compaction's `to_turn`, or 0 if none.
    /// Used by `--from last` for incremental compaction.
    AfterLastCompaction,
}

/// A resolved compaction range with inclusive bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionRange {
    /// First turn (inclusive, 0-based).
    pub from_turn: usize,
    /// Last turn (inclusive, 0-based).
    pub to_turn: usize,
}

/// Extend a summary compaction range to fully subsume any partially
/// overlapping existing summary compactions in the stream.
///
/// When two summary ranges partially overlap (each covers turns the other
/// doesn't), the projected view produces two synthetic pairs instead of one
/// coherent summary. This function prevents that by expanding the proposed
/// range to cover any such partial overlaps.
///
/// The extension repeats until no partial overlaps remain, handling transitive
/// chains (A overlaps B, B overlaps C → extend to cover all three).
///
/// Only considers existing compactions that have `summary: Some(...)`. Returns
/// the input range unchanged if there are no overlapping summaries.
///
/// Call this before generating the summary text so the summarizer reads
/// events for the full extended range.
#[must_use]
pub fn extend_summary_range(
    stream: &crate::ConversationStream,
    range: CompactionRange,
) -> CompactionRange {
    let mut from = range.from_turn;
    let mut to = range.to_turn;

    // Repeat until stable — extension may expose new overlaps.
    loop {
        let mut changed = false;

        for c in stream.compactions() {
            if c.summary.is_none() {
                continue;
            }

            let intersects = from <= c.to_turn && to >= c.from_turn;
            let new_contains_old = from <= c.from_turn && to >= c.to_turn;
            let old_contains_new = c.from_turn <= from && c.to_turn >= to;

            // Only extend on partial overlap: ranges intersect but neither
            // fully contains the other.
            if intersects && !new_contains_old && !old_contains_new {
                from = from.min(c.from_turn);
                to = to.max(c.to_turn);
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    CompactionRange {
        from_turn: from,
        to_turn: to,
    }
}

/// Resolve user-specified range bounds against a conversation stream.
///
/// Returns `None` if the stream has no turns, or if the resolved range is
/// empty (`from > to`).
///
/// Defaults: `from` = turn 0, `to` = last turn.
#[must_use]
pub fn resolve_range(
    stream: &crate::ConversationStream,
    from: Option<RangeBound>,
    to: Option<RangeBound>,
) -> Option<CompactionRange> {
    let count = stream.turn_count();
    if count == 0 {
        return None;
    }
    let last = count - 1;

    let resolve = |bound: RangeBound| -> usize {
        match bound {
            RangeBound::Absolute(n) => n.min(last),
            RangeBound::FromEnd(n) => last.saturating_sub(n),
            RangeBound::AfterLastCompaction => stream
                .compactions()
                .map(|c| c.to_turn + 1)
                .max()
                .unwrap_or(0)
                .min(last),
        }
    };

    let from_turn = from.map_or(0, resolve);
    let to_turn = to.map_or(last, resolve);

    if from_turn > to_turn {
        return None;
    }

    Some(CompactionRange { from_turn, to_turn })
}

#[cfg(test)]
#[path = "compaction_tests.rs"]
mod tests;
