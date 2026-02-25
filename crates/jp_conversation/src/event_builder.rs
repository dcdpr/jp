//! Event accumulation for the query stream pipeline.
//!
//! The [`EventBuilder`] accumulates streamed `jp_llm::Event` chunks from the
//! LLM into complete [`ConversationEvent`]s. It uses index-based buffering
//! where each index represents one logical event.
//!
//! # Event Model
//!
//! LLM providers stream events with an index:
//!
//! ```text
//! Part { index: 0, ChatResponse::Reasoning("Let ") }
//! Part { index: 0, ChatResponse::Reasoning("me think") }
//! Flush { index: 0 }  → Reasoning complete, pushed to stream
//!
//! Part { index: 1, ChatResponse::Message("The ") }
//! Part { index: 1, ChatResponse::Message("answer") }
//! Flush { index: 1 }  → Message complete, pushed to stream
//! ```
//!
//! # Key Properties
//!
//! - **Index-based grouping**: Parts with the same index are accumulated
//!   together
//! - **Flush boundary**: `Flush { index }` signals that all parts for that
//!   index are complete and should be merged into a single `ConversationEvent`
//! - **Order preservation**: Flush events arrive in index order
//! - **Tool calls may be multi-part**: Providers that stream tool calls
//!   incrementally (e.g. Anthropic) emit an initial Part with name + id
//!   when the tool call starts, followed by a final Part with the parsed
//!   arguments once all JSON chunks have arrived. The Flush after the last
//!   Part marks the tool call as complete.

use std::collections::{HashMap, hash_map::Entry};

use indexmap::IndexMap;
use serde_json::{Map, Value};
use tracing::warn;

use crate::{
    ConversationEvent, EventKind,
    event::{ChatResponse, ToolCallRequest},
};

/// Accumulates streamed events into complete [`ConversationEvent`]s.
pub struct EventBuilder {
    /// Index-based buffers for accumulating partial events.
    buffers: HashMap<usize, IndexBuffer>,

    /// Metadata accumulated from `jp_llm::Event::Part`s, keyed by stream index.
    metadata: HashMap<usize, Map<String, Value>>,
}

impl EventBuilder {
    /// Creates a new empty event builder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buffers: HashMap::new(),
            metadata: HashMap::new(),
        }
    }

    /// Returns the partial content accumulated in unflushed buffers.
    ///
    /// This is used when the user interrupts streaming and chooses to continue
    /// with assistant prefill. The partial content is injected into the next
    /// request so the LLM can continue from where it left off.
    ///
    /// Returns `None` if there's no meaningful partial content. Structured
    /// buffers are excluded — partial JSON isn't useful for prefill.
    #[must_use]
    pub fn peek_partial_content(&self) -> Option<String> {
        if self.buffers.is_empty() {
            return None;
        }

        // Collect content from all buffers, sorted by index for deterministic
        // output
        let mut indices: Vec<_> = self.buffers.keys().copied().collect();
        indices.sort_unstable();

        let mut parts = Vec::new();
        for index in indices {
            if let Some(buffer) = self.buffers.get(&index) {
                match buffer {
                    IndexBuffer::Reasoning { content } | IndexBuffer::Message { content }
                        if !content.is_empty() =>
                    {
                        parts.push(content.clone());
                    }
                    // Tool calls, structured buffers, empty content - skip
                    _ => {}
                }
            }
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(""))
        }
    }

    /// Handles a partial event from the LLM stream.
    ///
    /// Accumulates the event content into the buffer for the given index.
    pub fn handle_part(&mut self, index: usize, event: ConversationEvent) {
        // Accumulate metadata from each part (e.g. thinking signatures).
        if !event.metadata.is_empty() {
            self.metadata
                .entry(index)
                .or_default()
                .extend(event.metadata);
        }

        match event.kind {
            EventKind::ChatResponse(ChatResponse::Reasoning { reasoning }) => {
                match self.buffers.entry(index) {
                    Entry::Occupied(mut e) => match e.get_mut().as_reasoning_mut() {
                        Some(content) => content.push_str(&reasoning),
                        None => warn_mismatch(e.get(), "Reasoning"),
                    },
                    Entry::Vacant(e) => {
                        e.insert(IndexBuffer::Reasoning { content: reasoning });
                    }
                }
            }
            EventKind::ChatResponse(ChatResponse::Message { message }) => {
                match self.buffers.entry(index) {
                    Entry::Occupied(mut e) => match e.get_mut().as_message_mut() {
                        Some(content) => content.push_str(&message),
                        None => warn_mismatch(e.get(), "Message"),
                    },
                    Entry::Vacant(e) => {
                        e.insert(IndexBuffer::Message { content: message });
                    }
                }
            }
            EventKind::ToolCallRequest(request) => match self.buffers.entry(index) {
                Entry::Occupied(mut e) => e.get_mut().merge_tool_call(request),
                Entry::Vacant(e) => {
                    e.insert(IndexBuffer::ToolCall { request });
                }
            },
            EventKind::ChatResponse(ChatResponse::Structured { data }) => {
                let Value::String(chunk) = data else {
                    warn!("Structured part with non-string data; ignoring");
                    return;
                };

                match self.buffers.entry(index) {
                    Entry::Occupied(mut e) => match e.get_mut().as_structured_mut() {
                        Some(content) => content.push_str(&chunk),
                        None => warn_mismatch(e.get(), "Structured"),
                    },
                    Entry::Vacant(e) => {
                        e.insert(IndexBuffer::Structured { content: chunk });
                    }
                }
            }
            EventKind::ChatRequest(_)
            | EventKind::ToolCallResponse(_)
            | EventKind::InquiryRequest(_)
            | EventKind::InquiryResponse(_)
            | EventKind::TurnStart(_) => {}
        }
    }

    /// Flushes the buffer for the given index, producing a complete
    /// [`ConversationEvent`].
    ///
    /// Returns `None` if the index had no buffered content (or was a
    /// whitespace-only message that was dropped).
    pub fn handle_flush(
        &mut self,
        index: usize,
        metadata: IndexMap<String, Value>,
    ) -> Option<ConversationEvent> {
        let buffer = self.buffers.remove(&index)?;

        let mut event = match buffer {
            IndexBuffer::Reasoning { content } => {
                ConversationEvent::now(ChatResponse::Reasoning { reasoning: content })
            }
            // Skip whitespace-only messages. These appear when the LLM
            // emits blank text content blocks (e.g. "\n\n" between
            // interleaved thinking blocks).
            IndexBuffer::Message { content } if content.trim().is_empty() => return None,
            IndexBuffer::Message { content } => {
                ConversationEvent::now(ChatResponse::Message { message: content })
            }
            IndexBuffer::ToolCall { request } => ConversationEvent::now(request),
            IndexBuffer::Structured { content } => {
                let data = serde_json::from_str::<Value>(&content).unwrap_or_else(|e| {
                    warn!("Failed to parse structured response JSON: {e}");
                    Value::String(content)
                });
                ConversationEvent::now(ChatResponse::Structured { data })
            }
        };

        // Merge metadata accumulated from Part events (e.g. thinking
        // signatures that arrive via SignatureDelta).
        if let Some(part_metadata) = self.metadata.remove(&index) {
            event.metadata.extend(part_metadata);
        }

        // Merge metadata from the Flush event itself.
        event.metadata.extend(metadata);

        Some(event)
    }

    /// Flushes all remaining buffers.
    ///
    /// This is used when the stream ends (e.g. on `jp_llm::Event::Finished`) to
    /// ensure any partially accumulated events are not silently dropped.
    #[expect(
        clippy::needless_collect,
        reason = "collect breaks the borrow on self.buffers"
    )]
    pub fn drain(&mut self) -> Vec<ConversationEvent> {
        let indices: Vec<usize> = self.buffers.keys().copied().collect();
        indices
            .into_iter()
            .filter_map(|index| self.handle_flush(index, IndexMap::new()))
            .collect()
    }
}

impl Default for EventBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Buffer for accumulating partial events by type.
enum IndexBuffer {
    /// Accumulates reasoning content.
    Reasoning {
        /// The reasoning content accumulated so far.
        content: String,
    },
    /// Accumulates message content.
    Message {
        /// The message content accumulated so far.
        content: String,
    },
    /// Accumulates a tool call request (may be multi-part).
    ToolCall {
        /// The tool call request accumulated so far.
        request: ToolCallRequest,
    },
    /// Accumulates streamed JSON chunks for a structured response.
    ///
    /// During streaming, providers emit `ChatResponse::Structured` parts
    /// with `Value::String` chunks. On flush, the concatenated string is
    /// parsed into a `Value`. If parsing fails, the raw string is preserved.
    Structured {
        /// The JSON string accumulated so far.
        content: String,
    },
}

impl IndexBuffer {
    /// Merges an incoming tool call request into this buffer.
    ///
    /// First non-empty value wins for id and name; arguments are extended.
    /// Logs a warning if the buffer is not a `ToolCall`.
    fn merge_tool_call(&mut self, incoming: ToolCallRequest) {
        let Self::ToolCall { request } = self else {
            warn!(
                buffer_type = self.as_str(),
                "Expected ToolCall buffer; ignoring merge."
            );
            return;
        };

        if request.id.is_empty() && !incoming.id.is_empty() {
            request.id = incoming.id;
        }
        if request.name.is_empty() && !incoming.name.is_empty() {
            request.name = incoming.name;
        }
        request.arguments.extend(incoming.arguments);
    }

    /// Returns a mutable reference to the reasoning buffer content, if any.
    const fn as_reasoning_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::Reasoning { content } => Some(content),
            _ => None,
        }
    }

    /// Returns a mutable reference to the message buffer content, if any.
    const fn as_message_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::Message { content } => Some(content),
            _ => None,
        }
    }

    /// Returns a mutable reference to the structured buffer content, if any.
    const fn as_structured_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::Structured { content } => Some(content),
            _ => None,
        }
    }

    /// Returns the name of the buffer type.
    #[must_use]
    const fn as_str(&self) -> &str {
        match self {
            Self::Reasoning { .. } => "Reasoning",
            Self::Message { .. } => "Message",
            Self::ToolCall { .. } => "ToolCall",
            Self::Structured { .. } => "Structured",
        }
    }
}

/// Logs a warning when a part's type doesn't match the existing buffer.
fn warn_mismatch(buffer: &IndexBuffer, incoming: &str) {
    warn!(
        buffer_type = buffer.as_str(),
        incoming_type = incoming,
        "Mismatched event type for index; ignoring."
    );
}

#[cfg(test)]
#[path = "event_builder_tests.rs"]
mod tests;
