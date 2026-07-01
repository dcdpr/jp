//! Event accumulation for the query stream pipeline.
//!
//! The [`EventBuilder`] accumulates streamed [`EventPart`] chunks from the LLM
//! into complete [`ConversationEvent`]s.
//! It uses index-based buffering where each index represents one logical event.
//!
//! # Event Model
//!
//! LLM providers stream events with an index:
//!
//! ```text
//! Part { index: 0, Reasoning("Let ") }
//! Part { index: 0, Reasoning("me think") }
//! Flush { index: 0 }  → Reasoning complete, pushed to stream
//!
//! Part { index: 1, Message("The ") }
//! Part { index: 1, Message("answer") }
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
//!   incrementally (e.g. Anthropic) emit `ToolCallPart::Start` when the tool
//!   call begins, followed by `ToolCallPart::ArgumentChunk` events as JSON
//!   arrives.
//!   The Flush after the last chunk marks the tool call as complete.

use std::collections::{HashMap, hash_map::Entry};

use jp_conversation::{
    ConversationEvent,
    event::{ChatResponse, ToolCallRequest},
};
use serde_json::{Map, Value};
use tracing::warn;

use crate::event::{EventPart, ToolCallPart};

/// Accumulates streamed events into complete [`ConversationEvent`]s.
pub struct EventBuilder {
    /// Index-based buffers for accumulating partial events.
    buffers: HashMap<usize, IndexBuffer>,

    /// Metadata accumulated from streaming parts, keyed by stream index.
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

    /// Returns the partial assistant content accumulated in unflushed buffers,
    /// as correctly-typed responses in stream-index order.
    ///
    /// Used when a stream is interrupted or retried and the turn resumes: each
    /// partial buffer is committed as its own event, so reasoning stays
    /// reasoning and message text stays message text.
    /// The provider's request builder decides how to serialize partial
    /// reasoning back to the model (Anthropic sends unsigned reasoning as
    /// `<think>` text, never a native thinking block).
    ///
    /// Empty buffers are skipped.
    /// Tool-call and structured buffers are excluded: a partial tool call would
    /// orphan a `tool_use`, and partial structured JSON is not a usable resume
    /// point.
    #[must_use]
    pub fn peek_partial_events(&self) -> Vec<ChatResponse> {
        let mut indices: Vec<_> = self.buffers.keys().copied().collect();
        indices.sort_unstable();

        indices
            .into_iter()
            .filter_map(|index| match self.buffers.get(&index)? {
                IndexBuffer::Reasoning { content } if !content.is_empty() => {
                    Some(ChatResponse::Reasoning {
                        reasoning: content.clone(),
                    })
                }
                IndexBuffer::Message { content } if !content.is_empty() => {
                    Some(ChatResponse::Message {
                        message: content.clone(),
                    })
                }
                _ => None,
            })
            .collect()
    }

    /// Handles a streaming chunk from the LLM.
    ///
    /// Accumulates the event content into the buffer for the given index.
    pub fn handle_part(&mut self, index: usize, part: EventPart, metadata: Map<String, Value>) {
        // Accumulate metadata from each part (e.g. thinking signatures).
        if !metadata.is_empty() {
            self.metadata.entry(index).or_default().extend(metadata);
        }

        match part {
            EventPart::Reasoning(reasoning) => match self.buffers.entry(index) {
                Entry::Occupied(mut e) => match e.get_mut().as_reasoning_mut() {
                    Some(content) => content.push_str(&reasoning),
                    None => warn_mismatch(e.get(), "Reasoning"),
                },
                Entry::Vacant(e) => {
                    e.insert(IndexBuffer::Reasoning { content: reasoning });
                }
            },
            EventPart::Message(message) => match self.buffers.entry(index) {
                Entry::Occupied(mut e) => match e.get_mut().as_message_mut() {
                    Some(content) => content.push_str(&message),
                    None => warn_mismatch(e.get(), "Message"),
                },
                Entry::Vacant(e) => {
                    e.insert(IndexBuffer::Message { content: message });
                }
            },
            EventPart::ToolCall(tool_call_part) => match self.buffers.entry(index) {
                Entry::Occupied(mut e) => e.get_mut().merge_tool_call_part(tool_call_part),
                Entry::Vacant(e) => {
                    let buffer = match tool_call_part {
                        ToolCallPart::Start { id, name } => IndexBuffer::ToolCall {
                            id,
                            name,
                            arguments_json: String::new(),
                        },
                        ToolCallPart::ArgumentChunk(json) => IndexBuffer::ToolCall {
                            id: String::new(),
                            name: String::new(),
                            arguments_json: json,
                        },
                    };
                    e.insert(buffer);
                }
            },
            EventPart::Structured(chunk) => match self.buffers.entry(index) {
                Entry::Occupied(mut e) => match e.get_mut().as_structured_mut() {
                    Some(content) => content.push_str(&chunk),
                    None => warn_mismatch(e.get(), "Structured"),
                },
                Entry::Vacant(e) => {
                    e.insert(IndexBuffer::Structured { content: chunk });
                }
            },
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
        metadata: Map<String, Value>,
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
            IndexBuffer::ToolCall {
                id,
                name,
                arguments_json,
            } => {
                let arguments = if arguments_json.trim().is_empty() {
                    serde_json::Map::new()
                } else {
                    serde_json::from_str(&arguments_json).unwrap_or_else(|e| {
                        warn!("Failed to parse tool call arguments JSON: {e}");
                        serde_json::Map::new()
                    })
                };
                ConversationEvent::now(ToolCallRequest {
                    id,
                    name,
                    arguments,
                })
            }
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
    /// This is used when the stream ends (e.g. on [`Event::Finished`]) to
    /// ensure any partially accumulated events are not silently dropped.
    ///
    /// Tool-call buffers are an exception: a normally-completed tool call
    /// always emits an explicit [`Event::Flush`] (e.g. Anthropic's
    /// `ContentBlockStop`).
    /// A buffer that only reaches drain is structurally incomplete — the
    /// stream ended before the block was closed.
    /// Persisting it would create an orphaned `tool_use` in the conversation,
    /// which providers reject on the next request because there's no matching
    /// `tool_result`.
    /// Drop these with a warning.
    ///
    /// [`Event::Finished`]: crate::event::Event::Finished
    /// [`Event::Flush`]: crate::event::Event::Flush
    #[expect(
        clippy::needless_collect,
        reason = "collect breaks the borrow on self.buffers"
    )]
    pub fn drain(&mut self) -> Vec<ConversationEvent> {
        let indices: Vec<usize> = self.buffers.keys().copied().collect();
        indices
            .into_iter()
            .filter_map(|index| {
                if let Some(IndexBuffer::ToolCall { id, name, .. }) = self.buffers.get(&index) {
                    warn!(
                        index,
                        id, name, "Dropping incomplete tool call buffer at stream end."
                    );
                    self.buffers.remove(&index);
                    self.metadata.remove(&index);
                    return None;
                }
                self.handle_flush(index, Map::new())
            })
            .collect()
    }

    /// Names of tool-call buffers that are still incomplete.
    ///
    /// These are the buffers [`drain`] discards when a stream ends
    /// mid-tool-call.
    /// Query this before draining to report what was lost.
    /// Names are sorted for deterministic output.
    ///
    /// Unnamed buffers are skipped.
    /// A malformed stream can open a tool-call buffer from an argument chunk
    /// that arrives before its `Start`, which leaves both the name and id
    /// empty; reporting `""` would render an empty tool name in the diagnostic,
    /// so those buffers are dropped here rather than surfaced.
    /// [`drain`] still logs them.
    ///
    /// [`drain`]: Self::drain
    #[must_use]
    pub fn incomplete_tool_calls(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .buffers
            .values()
            .filter_map(|buffer| match buffer {
                IndexBuffer::ToolCall { name, .. } if !name.is_empty() => Some(name.clone()),
                _ => None,
            })
            .collect();
        names.sort();
        names
    }

    /// Discard all buffered partial events without producing any
    /// [`ConversationEvent`]s.
    ///
    /// Used when a stream ends in a way that invalidates partial output (a
    /// refusal): the buffered content must be dropped rather than flushed.
    pub fn clear(&mut self) {
        self.buffers.clear();
        self.metadata.clear();
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
    /// Accumulates a tool call request (identity + argument JSON chunks).
    ToolCall {
        /// Tool call ID.
        /// First non-empty value wins.
        id: String,
        /// Tool name.
        /// First non-empty value wins.
        name: String,
        /// Raw JSON arguments accumulated from chunks.
        arguments_json: String,
    },
    /// Accumulates streamed JSON chunks for a structured response.
    ///
    /// During streaming, providers emit `EventPart::Structured` chunks.
    /// On flush, the concatenated string is parsed into a `Value`.
    /// If parsing fails, the raw string is preserved.
    Structured {
        /// The JSON string accumulated so far.
        content: String,
    },
}

impl IndexBuffer {
    /// Merges an incoming tool call part into this buffer.
    fn merge_tool_call_part(&mut self, part: ToolCallPart) {
        let Self::ToolCall {
            id,
            name,
            arguments_json,
        } = self
        else {
            warn!(
                buffer_type = self.as_str(),
                "Expected ToolCall buffer; ignoring merge."
            );
            return;
        };

        match part {
            ToolCallPart::Start {
                id: incoming_id,
                name: incoming_name,
            } => {
                if id.is_empty() && !incoming_id.is_empty() {
                    *id = incoming_id;
                }
                if name.is_empty() && !incoming_name.is_empty() {
                    *name = incoming_name;
                }
            }
            ToolCallPart::ArgumentChunk(json) => {
                arguments_json.push_str(&json);
            }
        }
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
