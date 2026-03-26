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
    /// Checks that response events have matching requests already in the stream
    /// (or earlier in the buffer), and that no duplicate responses exist.
    ///
    /// All checks are scoped to the **current turn** — events from earlier
    /// turns are ignored. This is correct because request-response pairing
    /// is inherently turn-local, and providers like Google may reuse
    /// synthetic IDs across turns.
    ///
    /// Validation uses **count-based matching**: a response is valid when the
    /// number of responses with that ID is still below the number of requests
    /// with the same ID. This handles providers like Google Gemini that reuse
    /// the same tool call ID across multiple streaming cycles within a single
    /// turn.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - A [`ToolCallResponse`] has no matching [`ToolCallRequest`]
    /// - A [`ToolCallResponse`] would exceed the number of matching requests
    /// - An [`InquiryResponse`] has no matching [`InquiryRequest`]
    /// - An [`InquiryResponse`] would exceed the number of matching requests
    pub fn build(self) -> Result<(), StreamError> {
        let Self { stream, events } = self;
        let turn_events = {
            let events: &[InternalEvent] = &stream.events;
            let last_turn_start = events
                .iter()
                .rposition(|e| matches!(e, InternalEvent::Event(ev) if ev.is_turn_start()));

            last_turn_start.map_or(events, |pos| &events[pos..])
        };

        for (i, event) in events.iter().enumerate() {
            let events = &events[..i];

            match &event.kind {
                EventKind::ToolCallResponse(resp) => {
                    let id = &resp.id;

                    let requests = turn_events
                        .iter()
                        .filter_map(InternalEvent::as_event)
                        .chain(events.iter())
                        .filter_map(ConversationEvent::as_tool_call_request)
                        .filter(|r| r.id == *id)
                        .count();

                    if requests == 0 {
                        return Err(StreamError::OrphanedToolCallResponse { id: id.clone() });
                    }

                    let responses = turn_events
                        .iter()
                        .filter_map(InternalEvent::as_event)
                        .chain(events.iter())
                        .filter_map(ConversationEvent::as_tool_call_response)
                        .filter(|r| r.id == *id)
                        .count();

                    if responses >= requests {
                        return Err(StreamError::DuplicateToolCallResponse { id: id.clone() });
                    }
                }
                EventKind::InquiryResponse(resp) => {
                    let id = &resp.id;

                    let requests = turn_events
                        .iter()
                        .filter_map(InternalEvent::as_event)
                        .chain(events.iter())
                        .filter_map(ConversationEvent::as_inquiry_request)
                        .filter(|r| r.id == *id)
                        .count();

                    if requests == 0 {
                        return Err(StreamError::OrphanedInquiryResponse { id: id.to_string() });
                    }

                    let responses = turn_events
                        .iter()
                        .filter_map(InternalEvent::as_event)
                        .chain(events.iter())
                        .filter_map(ConversationEvent::as_inquiry_response)
                        .filter(|r| r.id == *id)
                        .count();

                    if responses >= requests {
                        return Err(StreamError::DuplicateInquiryResponse { id: id.to_string() });
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

#[cfg(test)]
#[path = "turn_mut_tests.rs"]
mod tests;
