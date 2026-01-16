//! Utilities related to conversation turns.
//!
//! See [`TurnState`] for more details.

use indexmap::{IndexMap, IndexSet};
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
    /// Tool answers that are instructed to be re-used for the duration of the
    /// turn.
    ///
    /// For example, if a tool `foo` asks a question `bar`, and the user
    /// indicates that the same answer should be used during this turn, then
    /// this map will contain a key `foo` with a value that contains a key `bar`
    /// with the [`Value`] of the answer.
    pub persisted_tool_answers: IndexMap<String, IndexMap<String, Value>>,

    /// The number of times we've tried a request to the assistant.
    ///
    /// This is used when the assistant returns an error that is retryable.
    /// Every retry increments this counter, until a maximum number of retries
    /// is reached, after which the turn ends in an error.
    pub request_count: usize,

    /// A list of pending tool call questions.
    ///
    /// The key is the [`ToolCallRequest::name`], the value is a list of
    /// question IDs that have not yet been answered.
    ///
    /// NOTE: In the future we could swap this to use `id` instead of `name`,
    /// but that requires either the LLM to correctly return the ID of the
    /// original tool call in the response, which might be fragile, or for us to
    /// track more tool call state, which is a bit more complex. Returning the
    /// name of the tool call is the simplest solution, and hasn't
    /// caused any issues so far.
    ///
    /// [`ToolCallRequest::name`]: jp_conversation::event::ToolCallRequest::name
    pub pending_tool_call_questions: IndexMap<String, IndexSet<String>>,
}
