mod chat;
mod config_delta;
mod inquiry;
mod tool_call;

use serde::{Deserialize, Serialize};
use time::UtcDateTime;

pub use self::{
    chat::{ChatRequest, ChatResponse},
    config_delta::ConfigDelta,
    inquiry::{InquiryAnswerType, InquiryQuestion, InquiryRequest, InquiryResponse, InquirySource},
    tool_call::{ToolCallRequest, ToolCallResponse},
};

/// A single event in a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationEvent {
    pub timestamp: UtcDateTime,
    #[serde(flatten)]
    pub kind: EventKind,
}

impl ConversationEvent {
    #[must_use]
    pub fn new(event: impl Into<EventKind>, timestamp: impl Into<UtcDateTime>) -> Self {
        Self {
            timestamp: timestamp.into(),
            kind: event.into(),
        }
    }

    #[must_use]
    pub fn now(event: impl Into<EventKind>) -> Self {
        Self::new(event, UtcDateTime::now())
    }

    #[must_use]
    pub fn is_request(&self) -> bool {
        matches!(
            self.kind,
            EventKind::ChatRequest(_)
                | EventKind::ToolCallRequest(_)
                | EventKind::InquiryRequest(_)
        )
    }

    #[must_use]
    pub fn is_response(&self) -> bool {
        matches!(
            self.kind,
            EventKind::ChatResponse(_)
                | EventKind::ToolCallResponse(_)
                | EventKind::InquiryResponse(_)
        )
    }

    #[must_use]
    pub fn is_chat_request(&self) -> bool {
        matches!(self.kind, EventKind::ChatRequest(_))
    }

    #[must_use]
    pub fn as_chat_request(&self) -> Option<&ChatRequest> {
        match &self.kind {
            EventKind::ChatRequest(request) => Some(request),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_chat_request_mut(&mut self) -> Option<&mut ChatRequest> {
        match &mut self.kind {
            EventKind::ChatRequest(request) => Some(request),
            _ => None,
        }
    }

    #[must_use]
    pub fn into_chat_request(self) -> Option<ChatRequest> {
        match self.kind {
            EventKind::ChatRequest(request) => Some(request),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_chat_response(&self) -> bool {
        matches!(self.kind, EventKind::ChatResponse(_))
    }

    #[must_use]
    pub fn as_chat_response(&self) -> Option<&ChatResponse> {
        match &self.kind {
            EventKind::ChatResponse(response) => Some(response),
            _ => None,
        }
    }

    #[must_use]
    pub fn into_chat_response(self) -> Option<ChatResponse> {
        match self.kind {
            EventKind::ChatResponse(response) => Some(response),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_tool_call_request(&self) -> bool {
        matches!(self.kind, EventKind::ToolCallRequest(_))
    }

    #[must_use]
    pub fn as_tool_call_request(&self) -> Option<&ToolCallRequest> {
        match &self.kind {
            EventKind::ToolCallRequest(request) => Some(request),
            _ => None,
        }
    }

    #[must_use]
    pub fn into_tool_call_request(self) -> Option<ToolCallRequest> {
        match self.kind {
            EventKind::ToolCallRequest(request) => Some(request),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_tool_call_response(&self) -> bool {
        matches!(self.kind, EventKind::ToolCallResponse(_))
    }

    #[must_use]
    pub fn as_tool_call_response(&self) -> Option<&ToolCallResponse> {
        match &self.kind {
            EventKind::ToolCallResponse(response) => Some(response),
            _ => None,
        }
    }

    #[must_use]
    pub fn into_tool_call_response(self) -> Option<ToolCallResponse> {
        match self.kind {
            EventKind::ToolCallResponse(response) => Some(response),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_inquiry_request(&self) -> bool {
        matches!(self.kind, EventKind::InquiryRequest(_))
    }

    #[must_use]
    pub fn as_inquiry_request(&self) -> Option<&InquiryRequest> {
        match &self.kind {
            EventKind::InquiryRequest(request) => Some(request),
            _ => None,
        }
    }

    #[must_use]
    pub fn into_inquiry_request(self) -> Option<InquiryRequest> {
        match self.kind {
            EventKind::InquiryRequest(request) => Some(request),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_inquiry_response(&self) -> bool {
        matches!(self.kind, EventKind::InquiryResponse(_))
    }

    #[must_use]
    pub fn as_inquiry_response(&self) -> Option<&InquiryResponse> {
        match &self.kind {
            EventKind::InquiryResponse(response) => Some(response),
            _ => None,
        }
    }

    #[must_use]
    pub fn into_inquiry_response(self) -> Option<InquiryResponse> {
        match self.kind {
            EventKind::InquiryResponse(response) => Some(response),
            _ => None,
        }
    }

    // #[must_use]
    // pub fn is_config_delta(&self) -> bool {
    //     matches!(self.kind, EventKind::ConfigDelta(_))
    // }
    //
    // #[must_use]
    // pub fn as_config_delta(&self) -> Option<&ConfigDelta> {
    //     match &self.kind {
    //         EventKind::ConfigDelta(delta) => Some(delta),
    //         _ => None,
    //     }
    // }
    //
    // #[must_use]
    // pub fn as_config_delta_mut(&mut self) -> Option<&mut ConfigDelta> {
    //     match &mut self.kind {
    //         EventKind::ConfigDelta(delta) => Some(delta),
    //         _ => None,
    //     }
    // }
    //
    // #[must_use]
    // pub fn into_config_delta(self) -> Option<ConfigDelta> {
    //     match self.kind {
    //         EventKind::ConfigDelta(delta) => Some(delta),
    //         _ => None,
    //     }
    // }
}

/// A type of event in a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    /// A chat request event.
    ///
    /// This event is usually triggered by the user, but can also be
    /// triggered automatically. It always originates from the client-side
    /// (e.g. the `jp` binary, or some other client).
    ChatRequest(ChatRequest),

    /// A chat response event.
    ///
    /// This event MUST be in response to a `ChatRequest` event. Multiple
    /// responses can be sent for a single request. This happens for example
    /// when the assistant reasons about the request before answering. The
    /// reasoning and answering are separate `ChatResponse` events.
    ChatResponse(ChatResponse),

    /// A tool call request event.
    ///
    /// This event is usually triggered by the assistant, but can also be
    /// triggered automatically by the client (e.g. the `jp` binary, or some
    /// other client).
    ToolCallRequest(ToolCallRequest),

    /// A tool call response event.
    ///
    /// This event MUST be in response to a `ToolCallRequest` event, and its
    /// `id` field MUST match the `id` field of the request.
    ToolCallResponse(ToolCallResponse),

    /// An inquiry request event.
    ///
    /// This event indicates that an inquiry is being made by either a tool,
    /// the assistant or the user to which *some entity* has to respond
    /// (using `InquiryResponse` with the proper answer).
    InquiryRequest(InquiryRequest),

    /// An inquiry response event.
    ///
    /// This event MUST be in response to an `InquiryRequest` event, and its
    /// `id` field MUST match the `id` field of the request.
    InquiryResponse(InquiryResponse),
}

impl From<ChatRequest> for EventKind {
    fn from(request: ChatRequest) -> Self {
        Self::ChatRequest(request)
    }
}

impl From<ChatResponse> for EventKind {
    fn from(response: ChatResponse) -> Self {
        Self::ChatResponse(response)
    }
}

impl From<ToolCallRequest> for EventKind {
    fn from(request: ToolCallRequest) -> Self {
        Self::ToolCallRequest(request)
    }
}

impl From<ToolCallResponse> for EventKind {
    fn from(response: ToolCallResponse) -> Self {
        Self::ToolCallResponse(response)
    }
}

impl From<InquiryRequest> for EventKind {
    fn from(request: InquiryRequest) -> Self {
        Self::InquiryRequest(request)
    }
}

impl From<InquiryResponse> for EventKind {
    fn from(response: InquiryResponse) -> Self {
        Self::InquiryResponse(response)
    }
}

// impl From<ConfigDelta> for EventKind {
//     fn from(delta: ConfigDelta) -> Self {
//         Self::ConfigDelta(delta)
//     }
// }
//
// impl From<PartialAppConfig> for EventKind {
//     fn from(config: PartialAppConfig) -> Self {
//         Self::ConfigDelta(ConfigDelta::new(config))
//     }
// }

impl From<ChatRequest> for ConversationEvent {
    fn from(request: ChatRequest) -> Self {
        Self::now(request)
    }
}

impl From<ChatResponse> for ConversationEvent {
    fn from(response: ChatResponse) -> Self {
        Self::now(response)
    }
}

impl From<ToolCallRequest> for ConversationEvent {
    fn from(request: ToolCallRequest) -> Self {
        Self::now(request)
    }
}

impl From<ToolCallResponse> for ConversationEvent {
    fn from(response: ToolCallResponse) -> Self {
        Self::now(response)
    }
}

impl From<InquiryRequest> for ConversationEvent {
    fn from(request: InquiryRequest) -> Self {
        Self::now(request)
    }
}

impl From<InquiryResponse> for ConversationEvent {
    fn from(response: InquiryResponse) -> Self {
        Self::now(response)
    }
}

// impl From<ConfigDelta> for ConversationEvent {
//     fn from(delta: ConfigDelta) -> Self {
//         Self::now(delta)
//     }
// }
//
// impl From<PartialAppConfig> for ConversationEvent {
//     fn from(config: PartialAppConfig) -> Self {
//         Self::now(ConfigDelta::new(config))
//     }
// }
