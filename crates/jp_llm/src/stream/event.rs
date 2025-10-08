use jp_conversation::message::ToolCallRequest;
use serde_json::Value;

/// Represents an event yielded by the chat completion stream.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A chunk of chat content or reasoning.
    ChatChunk(CompletionChunk),

    /// A request to call a tool.
    ToolCall(ToolCallRequest),

    /// Opaque provider-specific metadata.
    Metadata(String, Value),

    /// The stream ended.
    EndOfStream(StreamEndReason),
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamEndReason {
    /// The turn was completed by the assistant.
    Completed,

    /// The maximum number of tokens was reached before the assistant could
    /// complete the turn.
    MaxTokens,

    /// The assistant has stopped generating tokens for some reason.
    Other(String),
}

impl StreamEvent {
    #[must_use]
    pub fn metadata(key: impl Into<String>, value: impl Into<Value>) -> Self {
        Self::Metadata(key.into(), value.into())
    }

    #[must_use]
    pub fn into_chat_chunk(self) -> Option<CompletionChunk> {
        match self {
            Self::ChatChunk(chunk) => Some(chunk),
            _ => None,
        }
    }

    /// Returns `true` if the event is a tool call.
    #[must_use]
    pub fn is_tool_call(&self) -> bool {
        matches!(self, Self::ToolCall(_))
    }
}

/// A chunk of chat content or reasoning.
#[derive(Debug, Clone)]
pub enum CompletionChunk {
    /// Regular chat content.
    Content(String),

    /// Reasoning content.
    Reasoning(String),
}

impl CompletionChunk {
    #[must_use]
    pub fn into_content(self) -> Option<String> {
        match self {
            Self::Content(content) => Some(content),
            Self::Reasoning(_) => None,
        }
    }

    #[must_use]
    pub fn into_reasoning(self) -> Option<String> {
        match self {
            Self::Reasoning(reasoning) => Some(reasoning),
            Self::Content(_) => None,
        }
    }

    #[must_use]
    pub fn as_content_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::Content(content) => Some(content),
            Self::Reasoning(_) => None,
        }
    }
}
