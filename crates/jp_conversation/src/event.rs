use jp_config::{PartialAppConfig, PartialConfig};
use serde::{Deserialize, Serialize};
use time::UtcDateTime;
use tracing::warn;

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
    pub fn new(event: impl Into<EventKind>, timestamp: UtcDateTime) -> Self {
        Self {
            timestamp,
            kind: event.into(),
        }
    }

    #[must_use]
    pub fn now(event: impl Into<EventKind>) -> Self {
        Self::new(event, UtcDateTime::now())
    }

    #[must_use]
    pub fn config(&self) -> PartialAppConfig {
        match &self.kind {
            EventKind::ConfigDelta(config) => config.clone(),
            EventKind::UserMessage(_) | EventKind::AssistantMessage(_) => PartialAppConfig::empty(),
        }
    }

    #[must_use]
    pub fn is_user_message(&self) -> bool {
        matches!(self.kind, EventKind::UserMessage(_))
    }

    #[must_use]
    pub fn as_user_message(&self) -> Option<&UserMessage> {
        match &self.kind {
            EventKind::UserMessage(message) => Some(message),
            _ => None,
        }
    }

    #[must_use]
    pub fn into_user_message(self) -> Option<UserMessage> {
        match self.kind {
            EventKind::UserMessage(message) => Some(message),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_assistant_message(&self) -> bool {
        matches!(self.kind, EventKind::AssistantMessage(_))
    }

    #[must_use]
    pub fn as_assistant_message(&self) -> Option<&AssistantMessage> {
        match &self.kind {
            EventKind::AssistantMessage(message) => Some(message),
            _ => None,
        }
    }

    #[must_use]
    pub fn into_assistant_message(self) -> Option<AssistantMessage> {
        match self.kind {
            EventKind::AssistantMessage(message) => Some(message),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_config_delta(&self) -> bool {
        matches!(self.kind, EventKind::ConfigDelta(_))
    }

    #[must_use]
    pub fn as_config_delta(&self) -> Option<&PartialAppConfig> {
        match &self.kind {
            EventKind::ConfigDelta(config) => Some(config),
            _ => None,
        }
    }

    #[must_use]
    pub fn into_config_delta(self) -> Option<PartialAppConfig> {
        match self.kind {
            EventKind::ConfigDelta(config) => Some(config),
            _ => None,
        }
    }
}

/// Return the configuration for a conversation from all
/// [`EventKind::ConfigDelta`] events.
///
/// We start at the first event, and then merge any subsequent events into the
/// final [`PartialAppConfig`].
pub fn conversation_config(events: &[ConversationEvent]) -> PartialAppConfig {
    events
        .iter()
        .map(ConversationEvent::config)
        .reduce(|mut a, b| {
            if let Err(error) = a.merge(&(), b) {
                warn!(?error, "Failed to merge configuration partial.");
            }

            a
        })
        .unwrap_or_else(PartialAppConfig::empty)
}

/// A type of event in a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[expect(clippy::large_enum_variant)]
pub enum EventKind {
    /// A user message event.
    UserMessage(UserMessage),

    /// An assistant message event.
    AssistantMessage(AssistantMessage),

    /// A configuration delta event.
    ///
    /// This event is emitted when the configuration of the conversation changes
    /// compared to the last `ConfigDelta` event.
    ConfigDelta(PartialAppConfig),
    // TODO
    // /// A chat request event.
    // ///
    // /// This event is usually triggered by the user, but can also be
    // /// triggered automatically. It always originates from the client-side
    // /// (e.g. the `jp` binary, or some other client).
    // ChatRequest(ChatRequest),
    //
    // /// A chat response event.
    // ///
    // /// This event MUST be in response to a `ChatRequest` event. Multiple
    // /// responses can be sent for a single request. This happens for example
    // /// when the assistant reasons about the request before answering. The
    // /// reasoning and answering are separate `ChatResponse` events.
    // ChatResponse(ChatResponse),
    //
    // /// A tool call request event.
    // ///
    // /// This event is usually triggered by the assistant, but can also be
    // /// triggered automatically by the client (e.g. the `jp` binary, or some
    // /// other client).
    // ///
    // ///
    // /// This event MUST be in response to a `ChatRequest` event, and its
    // /// `request_id` field MUST match the `request_id` field of the request.
    // ToolCallRequest(ToolCallRequest),
    //
    // /// A tool call response event.
    // ///
    // /// This event MUST be in response to a `ToolCallRequest` event, and its
    // /// `request_id` field MUST match the `request_id` field of the request.
    // ToolCallResponse(ToolCallResponse),
    //
    // /// An inquiry request event.
    // ///
    // /// This event indicates that an inquiry is being made by either a tool,
    // /// the assistant or the user to which *some entity* has to respond
    // (using `InquiryResponse` with the proper answer).
    // InquiryRequest(InquiryRequest),
    //
    // /// An inquiry response event.
    // ///
    // /// This event MUST be in response to an `InquiryRequest` event, and its
    // /// `request_id` field MUST match the `request_id` field of the request.
    // InquiryResponse(InquiryResponse),
    //
    // /// The configuration state of the conversation is updated.
    // ///
    // /// When this event is emitted, all subsequent events in the stream are
    // /// bound to the new configuration.
    // ///
    // /// This is a *delta* event, meaning that it is merged on top of all
    // /// other `ConfigDelta` events in the stream.
    // ///
    // /// Any non-config events before the first `ConfigDelta` event are
    // /// considered to have the default configuration.
    // ConfigDelta(ConfigDelta),
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

impl From<PartialAppConfig> for EventKind {
    fn from(config: PartialAppConfig) -> Self {
        Self::ConfigDelta(config)
    }
}

impl From<PartialAppConfig> for ConversationEvent {
    fn from(config: PartialAppConfig) -> Self {
        Self::new(config, UtcDateTime::now())
    }
}
