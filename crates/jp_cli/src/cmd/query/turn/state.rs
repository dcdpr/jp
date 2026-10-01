//! Utilities related to conversation turns.
//!
//! See [`TurnState`] for more details.

use indexmap::IndexMap;
use serde_json::Value;

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
    /// What the user asked to remember for the rest of the turn, per tool name.
    ///
    /// It applies to every later call to that tool, not only the call it was
    /// given for.
    pub tools: IndexMap<String, ToolMemory>,

    /// What the turn keeps per tool call id.
    ///
    /// Lives as long as the turn rather than the call's executor, so a call
    /// prepared again after a restart continues its own count.
    pub calls: IndexMap<String, CallMemory>,

    /// The number of times we've tried a request to the assistant.
    ///
    /// This is used when the assistant returns an error that is retryable.
    /// Every retry increments this counter, until a maximum number of retries
    /// is reached, after which the turn ends in an error.
    pub request_count: usize,
}

/// What the user asked to remember about one tool for the rest of the turn.
#[derive(Debug, Default)]
pub struct ToolMemory {
    /// A permission decision given with "remember for this turn".
    ///
    /// `true` runs later calls without asking; `false` skips them.
    pub permission: Option<bool>,

    /// Answers given with "remember for this turn", keyed by question id.
    ///
    /// Never holds the answer to a secret question.
    pub answers: IndexMap<String, Value>,
}

/// What the turn keeps about one tool call.
#[derive(Debug, Default)]
pub struct CallMemory {
    /// How many times each question has been recorded for this call, keyed by
    /// question id.
    ///
    /// Numbers the call's inquiry ids, so each recorded question is unique
    /// within the turn.
    pub inquiry_attempts: IndexMap<String, usize>,
}

impl TurnState {
    /// The permission decision remembered for `tool_name`, if any.
    #[must_use]
    pub fn remembered_permission(&self, tool_name: &str) -> Option<bool> {
        self.tools.get(tool_name)?.permission
    }

    /// Remember `run` as the permission decision for every later call to
    /// `tool_name` this turn.
    pub fn remember_permission(&mut self, tool_name: &str, run: bool) {
        self.tools
            .entry(tool_name.to_owned())
            .or_default()
            .permission = Some(run);
    }

    /// The answer remembered for `tool_name`'s question `question_id`, if any.
    #[must_use]
    pub fn remembered_answer(&self, tool_name: &str, question_id: &str) -> Option<&Value> {
        self.tools.get(tool_name)?.answers.get(question_id)
    }

    /// Remember `answer` for `tool_name`'s question `question_id` for the rest
    /// of the turn.
    ///
    /// The caller keeps secret answers out of this.
    pub fn remember_answer(&mut self, tool_name: &str, question_id: &str, answer: Value) {
        self.tools
            .entry(tool_name.to_owned())
            .or_default()
            .answers
            .insert(question_id.to_owned(), answer);
    }

    /// Allocate the next 1-indexed attempt for a `(tool_call_id, question_id)`
    /// pair within this turn.
    ///
    /// The first call for a key returns `1`; each subsequent call for the same
    /// key returns the next integer.
    pub fn next_inquiry_attempt(&mut self, tool_call_id: &str, question_id: &str) -> usize {
        let attempt = self
            .calls
            .entry(tool_call_id.to_owned())
            .or_default()
            .inquiry_attempts
            .entry(question_id.to_owned())
            .or_insert(0);
        *attempt += 1;
        *attempt
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
