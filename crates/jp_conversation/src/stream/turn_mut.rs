//! See [`TurnMut`].

use tracing::warn;

use super::{ConversationStream, InternalEvent, StreamError};
use crate::{
    ConversationEvent, EventKind,
    event::{
        ChatRequest, ChatResponse, InquiryRequest, InquiryResponse, ToolCallRequest,
        ToolCallResponse,
    },
};

/// A mutable handle to the current turn in a [`ConversationStream`].
///
/// Events are buffered internally and flushed to the stream when [`build()`]
/// is called. This keeps the stream in a consistent state — partial or invalid
/// events never appear on the stream.
///
/// Two method styles for ergonomics:
///
/// - **`with_xxx(&mut self, x) -> &mut Self`** — borrowed, for chaining on an
///   existing binding.
/// - **`add_xxx(mut self, x) -> Self`** — owned, for fluent builder chains.
///
/// # Examples
///
/// ```ignore
/// // Borrowed style (existing binding):
/// let turn = stream.current_turn_mut();
/// turn.with_chat_response(response)
///     .with_tool_call_request(req)
///     .build()?;
///
/// // Owned style (one-liner):
/// stream.current_turn_mut()
///     .add_chat_response(response)
///     .add_tool_call_request(req)
///     .build()?;
/// ```
///
/// [`build()`]: TurnMut::build
#[must_use = "a `TurnMut` does nothing until `.build()` is called"]
pub struct TurnMut<'a> {
    /// The stream this handle mutates on [`build()`](TurnMut::build).
    stream: &'a mut ConversationStream,

    /// Buffered events to flush on [`build()`](TurnMut::build).
    events: Vec<ConversationEvent>,
}

impl<'a> TurnMut<'a> {
    /// Create a new `TurnMut` handle for the given stream.
    pub(crate) const fn new(stream: &'a mut ConversationStream) -> Self {
        Self {
            stream,
            events: Vec::new(),
        }
    }

    // -- Owned chainable methods (consume self, return Self) --

    /// Buffer a [`ChatRequest`] event.
    pub fn add_chat_request(mut self, request: impl Into<ChatRequest>) -> Self {
        self.events.push(ConversationEvent::now(request.into()));
        self
    }

    /// Buffer a [`ChatResponse`] event.
    pub fn add_chat_response(mut self, response: impl Into<ChatResponse>) -> Self {
        self.events.push(ConversationEvent::now(response.into()));
        self
    }

    /// Buffer a [`ToolCallRequest`] event.
    pub fn add_tool_call_request(mut self, request: impl Into<ToolCallRequest>) -> Self {
        self.events.push(ConversationEvent::now(request.into()));
        self
    }

    /// Buffer a [`ToolCallResponse`] event.
    pub fn add_tool_call_response(mut self, response: impl Into<ToolCallResponse>) -> Self {
        self.events.push(ConversationEvent::now(response.into()));
        self
    }

    /// Buffer an [`InquiryRequest`] event.
    pub fn add_inquiry_request(mut self, request: impl Into<InquiryRequest>) -> Self {
        self.events.push(ConversationEvent::now(request.into()));
        self
    }

    /// Buffer an [`InquiryResponse`] event.
    pub fn add_inquiry_response(mut self, response: impl Into<InquiryResponse>) -> Self {
        self.events.push(ConversationEvent::now(response.into()));
        self
    }

    /// Buffer an arbitrary [`ConversationEvent`].
    ///
    /// [`TurnStart`] events are silently dropped — use
    /// [`ConversationStream::start_turn`] to create turn boundaries.
    ///
    /// [`TurnStart`]: crate::event::TurnStart
    pub fn add_event(mut self, event: impl Into<ConversationEvent>) -> Self {
        let event = event.into();
        if event.is_turn_start() {
            warn!("TurnStart events cannot be added through TurnMut; use start_turn()");
            return self;
        }
        self.events.push(event);
        self
    }

    // -- Borrowed chainable methods (borrow self, return &mut Self) --

    /// Buffer a [`ChatRequest`] event.
    pub fn with_chat_request(&mut self, request: impl Into<ChatRequest>) -> &mut Self {
        self.events.push(ConversationEvent::now(request.into()));
        self
    }

    /// Buffer a [`ChatResponse`] event.
    pub fn with_chat_response(&mut self, response: impl Into<ChatResponse>) -> &mut Self {
        self.events.push(ConversationEvent::now(response.into()));
        self
    }

    /// Buffer a [`ToolCallRequest`] event.
    pub fn with_tool_call_request(&mut self, request: impl Into<ToolCallRequest>) -> &mut Self {
        self.events.push(ConversationEvent::now(request.into()));
        self
    }

    /// Buffer a [`ToolCallResponse`] event.
    pub fn with_tool_call_response(&mut self, response: impl Into<ToolCallResponse>) -> &mut Self {
        self.events.push(ConversationEvent::now(response.into()));
        self
    }

    /// Buffer an [`InquiryRequest`] event.
    pub fn with_inquiry_request(&mut self, request: impl Into<InquiryRequest>) -> &mut Self {
        self.events.push(ConversationEvent::now(request.into()));
        self
    }

    /// Buffer an [`InquiryResponse`] event.
    pub fn with_inquiry_response(&mut self, response: impl Into<InquiryResponse>) -> &mut Self {
        self.events.push(ConversationEvent::now(response.into()));
        self
    }

    /// Buffer an arbitrary [`ConversationEvent`].
    ///
    /// [`TurnStart`] events are silently dropped — use
    /// [`ConversationStream::start_turn`] to create turn boundaries.
    ///
    /// [`TurnStart`]: crate::event::TurnStart
    pub fn with_event(&mut self, event: impl Into<ConversationEvent>) -> &mut Self {
        let event = event.into();
        if event.is_turn_start() {
            warn!("TurnStart events cannot be added through TurnMut; use start_turn()");
            return self;
        }
        self.events.push(event);
        self
    }

    /// Validate and flush buffered events to the stream.
    ///
    /// Checks that response events have matching requests already in the
    /// stream (or earlier in the buffer), and that no duplicate responses
    /// exist.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - A [`ToolCallResponse`] has no matching [`ToolCallRequest`]
    /// - A [`ToolCallResponse`] duplicates an existing response ID
    /// - An [`InquiryResponse`] has no matching [`InquiryRequest`]
    /// - An [`InquiryResponse`] duplicates an existing response ID
    pub fn build(self) -> Result<(), StreamError> {
        let Self { stream, events } = self;

        for (i, event) in events.iter().enumerate() {
            match &event.kind {
                EventKind::ToolCallResponse(resp) => {
                    let id = &resp.id;

                    let has_request = stream_has_tool_call_request(&stream.events, id)
                        || buffer_has_tool_call_request(&events[..i], id);

                    if !has_request {
                        return Err(StreamError::OrphanedToolCallResponse { id: id.clone() });
                    }

                    let has_dup = stream_has_tool_call_response(&stream.events, id)
                        || buffer_has_tool_call_response(&events[..i], id);

                    if has_dup {
                        return Err(StreamError::DuplicateToolCallResponse { id: id.clone() });
                    }
                }
                EventKind::InquiryResponse(resp) => {
                    let id = &resp.id;

                    let has_request = stream_has_inquiry_request(&stream.events, id)
                        || buffer_has_inquiry_request(&events[..i], id);

                    if !has_request {
                        return Err(StreamError::OrphanedInquiryResponse {
                            id: id.to_string(),
                        });
                    }

                    let has_dup = stream_has_inquiry_response(&stream.events, id)
                        || buffer_has_inquiry_response(&events[..i], id);

                    if has_dup {
                        return Err(StreamError::DuplicateInquiryResponse {
                            id: id.to_string(),
                        });
                    }
                }
                _ => {}
            }
        }

        for event in events {
            stream.push(event);
        }
        Ok(())
    }
}

// -- Stream lookup helpers (scan InternalEvent vec directly) --

/// Whether the stream contains a `ToolCallRequest` with the given ID.
fn stream_has_tool_call_request(events: &[InternalEvent], id: &str) -> bool {
    events
        .iter()
        .filter_map(InternalEvent::as_event)
        .any(|e| e.as_tool_call_request().is_some_and(|r| r.id == id))
}

/// Whether the stream contains a `ToolCallResponse` with the given ID.
fn stream_has_tool_call_response(events: &[InternalEvent], id: &str) -> bool {
    events
        .iter()
        .filter_map(InternalEvent::as_event)
        .any(|e| e.as_tool_call_response().is_some_and(|r| r.id == id))
}

/// Whether the stream contains an `InquiryRequest` with the given ID.
fn stream_has_inquiry_request(events: &[InternalEvent], id: &crate::event::InquiryId) -> bool {
    events
        .iter()
        .filter_map(InternalEvent::as_event)
        .any(|e| e.as_inquiry_request().is_some_and(|r| r.id == *id))
}

/// Whether the stream contains an `InquiryResponse` with the given ID.
fn stream_has_inquiry_response(events: &[InternalEvent], id: &crate::event::InquiryId) -> bool {
    events
        .iter()
        .filter_map(InternalEvent::as_event)
        .any(|e| e.as_inquiry_response().is_some_and(|r| r.id == *id))
}

// -- Buffer lookup helpers (scan ConversationEvent slice) --

/// Whether earlier buffer events contain a `ToolCallRequest` with the given ID.
fn buffer_has_tool_call_request(events: &[ConversationEvent], id: &str) -> bool {
    events
        .iter()
        .any(|e| e.as_tool_call_request().is_some_and(|r| r.id == id))
}

/// Whether earlier buffer events contain a `ToolCallResponse` with the given ID.
fn buffer_has_tool_call_response(events: &[ConversationEvent], id: &str) -> bool {
    events
        .iter()
        .any(|e| e.as_tool_call_response().is_some_and(|r| r.id == id))
}

/// Whether earlier buffer events contain an `InquiryRequest` with the given ID.
fn buffer_has_inquiry_request(
    events: &[ConversationEvent],
    id: &crate::event::InquiryId,
) -> bool {
    events
        .iter()
        .any(|e| e.as_inquiry_request().is_some_and(|r| r.id == *id))
}

/// Whether earlier buffer events contain an `InquiryResponse` with the given ID.
fn buffer_has_inquiry_response(
    events: &[ConversationEvent],
    id: &crate::event::InquiryId,
) -> bool {
    events
        .iter()
        .any(|e| e.as_inquiry_response().is_some_and(|r| r.id == *id))
}

#[cfg(test)]
#[path = "turn_mut_tests.rs"]
mod tests;
