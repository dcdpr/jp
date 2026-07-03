//! Utilities related to conversation turns.
//!
//! See [`TurnState`] for more details.

use indexmap::IndexMap;
use serde_json::Value;

/// The reserved question-id segment used for a tool's permission prompt.
const PERMISSION_QUESTION_ID: &str = "__permission__";

/// Turn-cache key for a tool's remembered permission decision.
///
/// Format `"<tool_name>.__permission__"`, stable across invocations of the same
/// tool within a turn.
/// Deliberately a distinct type from the stream-correlation inquiry ID, which
/// identifies a persisted inquiry uniquely per attempt; the two must never be
/// used interchangeably.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PermissionCacheKey(String);

impl PermissionCacheKey {
    /// Builds the permission-decision key for `tool_name`.
    #[must_use]
    pub fn new(tool_name: &str) -> Self {
        Self(format!("{tool_name}.{PERMISSION_QUESTION_ID}"))
    }
}

/// Turn-cache key for a remembered tool-question answer.
///
/// Format `"<tool_name>.<question_id>"`, stable across invocations of the same
/// tool within a turn.
/// Deliberately a distinct type from the stream-correlation inquiry ID, which
/// identifies a persisted inquiry uniquely per attempt; the two must never be
/// used interchangeably.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ToolAnswerCacheKey(String);

impl ToolAnswerCacheKey {
    /// Builds the answer key for `tool_name`'s `question_id`.
    #[must_use]
    pub fn new(tool_name: &str, question_id: &str) -> Self {
        Self(format!("{tool_name}.{question_id}"))
    }
}

/// State that is persisted for the duration of a turn.
///
/// A turn is one or more request-response cycle(s) between the user and the
/// assistant.
///
/// A turn MUST be initiated by the user with a `ChatRequest`, which MUST be
/// followed by a `ChatResponse` and/or `ToolCallRequest` from the assistant.
///
/// After a `ToolCallRequest`, the user MUST return a `ToolCallResponse`, after
/// which the assistant MUST return a `ChatResponse` and/or a `ToolCallRequest`.
///
/// The turn CONTINUES as long as the assistant responds with at least one
/// `ToolCallRequest`.
///
/// The turn ENDS when the assistant responds with a `ChatResponse` but no
/// `ToolCallRequest`.
#[derive(Debug, Default)]
pub struct TurnState {
    /// Tool-question answers remembered for the duration of the turn.
    ///
    /// Written when a user answers a tool question with "remember for this
    /// turn", and read to auto-answer the same question on a later tool call
    /// within the turn.
    pub remembered_tool_answers: IndexMap<ToolAnswerCacheKey, Value>,

    /// Tool-permission decisions remembered for the duration of the turn.
    ///
    /// Gated by the permission prompt's `persist` flag.
    /// `true` runs the tool without prompting again; `false` skips it.
    pub remembered_permission_decisions: IndexMap<PermissionCacheKey, bool>,

    /// The number of times we've tried a request to the assistant.
    ///
    /// This is used when the assistant returns an error that is retryable.
    /// Every retry increments this counter, until a maximum number of retries
    /// is reached, after which the turn ends in an error.
    pub request_count: usize,

    /// Per-`(tool_call_id, question_id)` attempt counter for minting unique
    /// three-segment inquiry IDs within the turn.
    ///
    /// In-memory only; a fresh `TurnState` (built per turn) resets it.
    pub inquiry_attempts: IndexMap<(String, String), usize>,
}

impl TurnState {
    /// Allocate the next 1-indexed attempt for a `(tool_call_id, question_id)`
    /// pair within this turn.
    ///
    /// The first call for a key returns `1`; each subsequent call for the same
    /// key returns the next integer.
    pub fn next_inquiry_attempt(&mut self, tool_call_id: &str, question_id: &str) -> usize {
        let attempt = self
            .inquiry_attempts
            .entry((tool_call_id.to_owned(), question_id.to_owned()))
            .or_insert(0);
        *attempt += 1;
        *attempt
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
