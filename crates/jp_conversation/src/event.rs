use serde::{Deserialize, Serialize};
use time::UtcDateTime;

use crate::{AssistantMessage, UserMessage};

/// A single event in a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationEvent {
    pub timestamp: UtcDateTime,
    #[serde(flatten)]
    pub kind: EventKind,
}

impl ConversationEvent {
    #[must_use]
    pub fn new(event: impl Into<EventKind>) -> Self {
        Self {
            timestamp: UtcDateTime::now(),
            kind: event.into(),
        }
    }
}

/// A type of event in a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    /// A user message event.
    UserMessage(UserMessage),

    /// An assistant message event.
    AssistantMessage(AssistantMessage),
    // TODO
    // UserMessage(UserMessageEvent),
    // AssistantReasoning(AssistantReasoningEvent),
    // AssistantMessage(AssistantMessageEvent),
    // ToolCallRequest(ToolCallRequestEvent),
    // ToolCallResult(ToolCallResultEvent),
    // InquiryRequest(InquiryRequestEvent),
    // InquiryResult(InquiryResultEvent),
}

impl From<UserMessage> for EventKind {
    fn from(message: UserMessage) -> Self {
        Self::UserMessage(message)
    }
}

impl From<AssistantMessage> for EventKind {
    fn from(message: AssistantMessage) -> Self {
        Self::AssistantMessage(message)
    }
}
