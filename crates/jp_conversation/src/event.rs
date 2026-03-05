//! See [`ConversationEvent`] and [`EventKind`].

mod chat;
mod inquiry;
mod tool_call;
mod turn;

use std::fmt;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use self::{
    chat::{ChatRequest, ChatResponse},
    inquiry::{
        InquiryAnswerType, InquiryId, InquiryQuestion, InquiryRequest, InquiryResponse,
        InquirySource, SelectOption,
    },
    tool_call::{ToolCallRequest, ToolCallResponse},
    turn::TurnStart,
};

/// A single event in a conversation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationEvent {
    /// The timestamp of the event.
    #[serde(
        serialize_with = "crate::serialize_dt",
        deserialize_with = "crate::deserialize_dt"
    )]
    pub timestamp: DateTime<Utc>,

    /// The kind of event.
    #[serde(flatten)]
    pub kind: EventKind,

    /// Additional opaque metadata associated with the event.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

impl fmt::Debug for ConversationEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConversationEvent")
            .field("timestamp", &crate::DebugDt(&self.timestamp))
            .field("kind", &self.kind)
            .field("metadata", &self.metadata)
            .finish()
    }
}

impl ConversationEvent {
    /// Create a new event with the given timestamp and kind.
    #[must_use]
    pub fn new(event: impl Into<EventKind>, timestamp: impl Into<DateTime<Utc>>) -> Self {
        Self {
            timestamp: timestamp.into(),
            kind: event.into(),
            metadata: Map::new(),
        }
    }

    /// Create a new event with the current timestamp and kind.
    #[must_use]
    pub fn now(event: impl Into<EventKind>) -> Self {
        Self::new(event, Utc::now())
    }

    /// Attaches metadata to the event.
    #[must_use]
    pub fn with_metadata(mut self, metadata: impl Into<IndexMap<String, Value>>) -> Self {
        self.metadata.extend(metadata.into());
        self
    }

    /// Attaches a metadata field to the event.
    #[must_use]
    pub fn with_metadata_field(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Adds a metadata field to the event.
    pub fn add_metadata_field(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        self.metadata.insert(key.into(), value.into());
    }

    /// Returns `true` if the event is a "request".
    #[must_use]
    pub const fn is_request(&self) -> bool {
        matches!(
            self.kind,
            EventKind::ChatRequest(_)
                | EventKind::ToolCallRequest(_)
                | EventKind::InquiryRequest(_)
        )
    }

    /// Returns `true` if the event is a "response".
    #[must_use]
    pub const fn is_response(&self) -> bool {
        matches!(
            self.kind,
            EventKind::ChatResponse(_)
                | EventKind::ToolCallResponse(_)
                | EventKind::InquiryResponse(_)
        )
    }

    /// Returns `true` if the event is a [`ChatRequest`].
    #[must_use]
    pub const fn is_chat_request(&self) -> bool {
        matches!(self.kind, EventKind::ChatRequest(_))
    }

    /// Returns a reference to the [`ChatRequest`], if applicable.
    #[must_use]
    pub const fn as_chat_request(&self) -> Option<&ChatRequest> {
        match &self.kind {
            EventKind::ChatRequest(request) => Some(request),
            _ => None,
        }
    }

    /// Returns a mutable reference to the [`ChatRequest`], if applicable.
    #[must_use]
    pub const fn as_chat_request_mut(&mut self) -> Option<&mut ChatRequest> {
        match &mut self.kind {
            EventKind::ChatRequest(request) => Some(request),
            _ => None,
        }
    }

    /// Consumes the event and returns the [`ChatRequest`], if applicable.
    #[must_use]
    pub fn into_chat_request(self) -> Option<ChatRequest> {
        match self.kind {
            EventKind::ChatRequest(request) => Some(request),
            _ => None,
        }
    }

    /// Returns `true` if the event is a [`ChatResponse`].
    #[must_use]
    pub const fn is_chat_response(&self) -> bool {
        matches!(self.kind, EventKind::ChatResponse(_))
    }

    /// Returns a reference to the [`ChatResponse`], if applicable.
    #[must_use]
    pub const fn as_chat_response(&self) -> Option<&ChatResponse> {
        match &self.kind {
            EventKind::ChatResponse(response) => Some(response),
            _ => None,
        }
    }

    /// Returns a mutable reference to the [`ChatResponse`], if applicable.
    #[must_use]
    pub const fn as_chat_response_mut(&mut self) -> Option<&mut ChatResponse> {
        match &mut self.kind {
            EventKind::ChatResponse(response) => Some(response),
            _ => None,
        }
    }

    /// Consumes the event and returns the [`ChatResponse`], if applicable.
    #[must_use]
    pub fn into_chat_response(self) -> Option<ChatResponse> {
        match self.kind {
            EventKind::ChatResponse(response) => Some(response),
            _ => None,
        }
    }

    /// Returns `true` if the event is a [`ToolCallRequest`].
    #[must_use]
    pub const fn is_tool_call_request(&self) -> bool {
        matches!(self.kind, EventKind::ToolCallRequest(_))
    }

    /// Returns a reference to the [`ToolCallRequest`], if applicable.
    #[must_use]
    pub const fn as_tool_call_request(&self) -> Option<&ToolCallRequest> {
        match &self.kind {
            EventKind::ToolCallRequest(request) => Some(request),
            _ => None,
        }
    }

    /// Returns a mutable reference to the [`ToolCallRequest`], if applicable.
    #[must_use]
    pub const fn as_tool_call_request_mut(&mut self) -> Option<&mut ToolCallRequest> {
        match &mut self.kind {
            EventKind::ToolCallRequest(request) => Some(request),
            _ => None,
        }
    }

    /// Consumes the event and returns the [`ToolCallRequest`], if applicable.
    #[must_use]
    pub fn into_tool_call_request(self) -> Option<ToolCallRequest> {
        match self.kind {
            EventKind::ToolCallRequest(request) => Some(request),
            _ => None,
        }
    }

    /// Returns `true` if the event is a [`ToolCallResponse`].
    #[must_use]
    pub const fn is_tool_call_response(&self) -> bool {
        matches!(self.kind, EventKind::ToolCallResponse(_))
    }

    /// Returns a reference to the [`ToolCallResponse`], if applicable.
    #[must_use]
    pub const fn as_tool_call_response(&self) -> Option<&ToolCallResponse> {
        match &self.kind {
            EventKind::ToolCallResponse(response) => Some(response),
            _ => None,
        }
    }

    /// Returns a mutable reference to the [`ToolCallResponse`], if applicable.
    #[must_use]
    pub const fn as_tool_call_response_mut(&mut self) -> Option<&mut ToolCallResponse> {
        match &mut self.kind {
            EventKind::ToolCallResponse(response) => Some(response),
            _ => None,
        }
    }

    /// Consumes the event and returns the [`ToolCallResponse`], if applicable.
    #[must_use]
    pub fn into_tool_call_response(self) -> Option<ToolCallResponse> {
        match self.kind {
            EventKind::ToolCallResponse(response) => Some(response),
            _ => None,
        }
    }

    /// Returns `true` if the event is a [`InquiryRequest`].
    #[must_use]
    pub const fn is_inquiry_request(&self) -> bool {
        matches!(self.kind, EventKind::InquiryRequest(_))
    }

    /// Returns a reference to the [`InquiryRequest`], if applicable.
    #[must_use]
    pub const fn as_inquiry_request(&self) -> Option<&InquiryRequest> {
        match &self.kind {
            EventKind::InquiryRequest(request) => Some(request),
            _ => None,
        }
    }

    /// Returns a mutable reference to the [`InquiryRequest`], if applicable.
    #[must_use]
    pub const fn as_inquiry_request_mut(&mut self) -> Option<&mut InquiryRequest> {
        match &mut self.kind {
            EventKind::InquiryRequest(request) => Some(request),
            _ => None,
        }
    }

    /// Consumes the event and returns the [`InquiryRequest`], if applicable.
    #[must_use]
    pub fn into_inquiry_request(self) -> Option<InquiryRequest> {
        match self.kind {
            EventKind::InquiryRequest(request) => Some(request),
            _ => None,
        }
    }

    /// Returns `true` if the event is a [`InquiryResponse`].
    #[must_use]
    pub const fn is_inquiry_response(&self) -> bool {
        matches!(self.kind, EventKind::InquiryResponse(_))
    }

    /// Returns a reference to the [`InquiryResponse`], if applicable.
    #[must_use]
    pub const fn as_inquiry_response(&self) -> Option<&InquiryResponse> {
        match &self.kind {
            EventKind::InquiryResponse(response) => Some(response),
            _ => None,
        }
    }

    /// Returns a mutable reference to the [`InquiryResponse`], if applicable.
    #[must_use]
    pub const fn as_inquiry_response_mut(&mut self) -> Option<&mut InquiryResponse> {
        match &mut self.kind {
            EventKind::InquiryResponse(response) => Some(response),
            _ => None,
        }
    }

    /// Consumes the event and returns the [`InquiryResponse`], if applicable.
    #[must_use]
    pub fn into_inquiry_response(self) -> Option<InquiryResponse> {
        match self.kind {
            EventKind::InquiryResponse(response) => Some(response),
            _ => None,
        }
    }

    /// Returns `true` if the event is a [`TurnStart`].
    #[must_use]
    pub const fn is_turn_start(&self) -> bool {
        matches!(self.kind, EventKind::TurnStart(_))
    }

    /// Returns a reference to the [`TurnStart`], if applicable.
    #[must_use]
    pub const fn as_turn_start(&self) -> Option<&TurnStart> {
        match &self.kind {
            EventKind::TurnStart(turn_start) => Some(turn_start),
            _ => None,
        }
    }

    /// Consumes the event and returns the [`TurnStart`], if applicable.
    #[must_use]
    pub fn into_turn_start(self) -> Option<TurnStart> {
        match self.kind {
            EventKind::TurnStart(turn_start) => Some(turn_start),
            _ => None,
        }
    }
}

/// A type of event in a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    /// A turn start event.
    ///
    /// This event marks the beginning of a new turn in the conversation. A turn
    /// groups together a user's chat request through the assistant's final
    /// response, including any intermediate tool calls.
    TurnStart(TurnStart),

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

impl EventKind {
    /// Returns the name of the event kind.
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::TurnStart(_) => "TurnStart",
            Self::ChatRequest(_) => "ChatRequest",
            Self::ChatResponse(_) => "ChatResponse",
            Self::ToolCallRequest(_) => "ToolCallRequest",
            Self::ToolCallResponse(_) => "ToolCallResponse",
            Self::InquiryRequest(_) => "InquiryRequest",
            Self::InquiryResponse(_) => "InquiryResponse",
        }
    }
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

impl From<TurnStart> for EventKind {
    fn from(turn_start: TurnStart) -> Self {
        Self::TurnStart(turn_start)
    }
}

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

impl From<TurnStart> for ConversationEvent {
    fn from(turn_start: TurnStart) -> Self {
        Self::now(turn_start)
    }
}
