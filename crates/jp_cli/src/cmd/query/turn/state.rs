//! Utilities related to conversation turns.
//!
//! See [`TurnState`] for more details.

use indexmap::IndexMap;
use jp_conversation::event::{InquiryId, InquiryResponse};

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
    /// Inquiry responses remembered for the duration of the turn.
    ///
    /// The caller mints the [`InquiryId`] and decides the convention. For
    /// tool permission prompts this is `"<tool_name>.__permission__"`, for
    /// tool questions it's `"<tool_name>.<question_id>"`.
    pub persisted_inquiry_responses: IndexMap<InquiryId, InquiryResponse>,

    /// The number of times we've tried a request to the assistant.
    ///
    /// This is used when the assistant returns an error that is retryable.
    /// Every retry increments this counter, until a maximum number of retries
    /// is reached, after which the turn ends in an error.
    pub request_count: usize,
}
