use jp_conversation::{ConversationStream, OverlayAction, OverlayMatcher, OverlayPatch};
use serde_json::{Map, Value};

/// Represents a completed event from the LLM.
///
/// In the context of [`crate::Provider::chat_completion_stream`], individual
/// [`Event::Part`]s represent incomplete chunks of data streamed from the
/// provider.
///
/// These parts will have their `index` field set to the index of the final
/// messages array streamed by the provider.
/// For example, if the LLM produces reasoning tokens, response tokens and tool
/// call tokens, then all of these will have different `index` values
/// corresponding to their relative order.
///
/// Only after [`Event::Flush`] is produced with the given `index` value, should
/// all previous parts be merged into a single [`ConversationEvent`] (e.g. via
/// [`EventBuilder`]).
///
/// [`ConversationEvent`]: jp_conversation::ConversationEvent
/// [`EventBuilder`]: crate::event_builder::EventBuilder
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A streaming chunk from the LLM provider.
    Part {
        /// The index of the final message in the array of complete messages
        /// streamed by the provider.
        ///
        /// For example, if the LLM produces reasoning tokens, response tokens
        /// and tool call tokens, then the chunks for all of these will have
        /// different `index` values corresponding to their relative order.
        index: usize,

        /// The streaming data chunk.
        part: EventPart,

        /// Metadata accumulated during streaming (e.g. thinking signatures).
        metadata: Map<String, Value>,
    },

    /// Flush one or more [`Event::Part`]s.
    ///
    /// Can optionally carry final metadata (e.g. usage, signatures) to attach
    /// to the event before emitting it.
    Flush {
        /// The index of the [`Event::Part`]s to flush.
        index: usize,

        /// Additional opaque metadata associated with the event.
        metadata: Map<String, Value>,
    },

    /// Instruct the caller to patch the conversation stream based on the
    /// patching rules.
    ///
    /// This allows LLM providers to inform the caller about an invalid event
    /// stream that the provider can fix, but which should ideally also be
    /// applied to the conversation stream itself so that the error doesn't show
    /// up again in the future.
    ///
    /// Callers record these with [`record_patches`], which appends an overlay
    /// to the stream rather than rewriting the events it targets.
    /// The stored history stays byte-identical; the patches take effect when
    /// the stream is projected for a request.
    Patch(Vec<EventPatch>),

    /// The response was finished.
    Finished(FinishReason),

    /// A liveness signal from the provider, such as an SSE keep-alive ping.
    ///
    /// Carries no content and is never persisted or rendered.
    /// It signals only that the connection is still alive.
    KeepAlive,
}

/// A chunk of streaming data from an LLM provider.
///
/// Each variant maps to a distinct content type that providers differentiate
/// between.
/// The [`EventBuilder`] accumulates these into [`ConversationEvent`]s on
/// [`Event::Flush`].
///
/// [`ConversationEvent`]: jp_conversation::ConversationEvent
/// [`EventBuilder`]: crate::event_builder::EventBuilder
#[derive(Debug, Clone, PartialEq)]
pub enum EventPart {
    /// A chunk of assistant message content.
    Message(String),

    /// A chunk of reasoning/thinking content.
    Reasoning(String),

    /// A chunk of structured response JSON.
    Structured(String),

    /// Tool call streaming data.
    ToolCall(ToolCallPart),
}

/// Streaming events for a single tool call.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolCallPart {
    /// Tool call identity.
    /// First non-empty value wins per field when multiple Start events arrive
    /// for the same index.
    Start {
        /// Unique identifier for this tool call.
        id: String,

        /// Name of the tool to execute.
        name: String,
    },

    /// A raw JSON chunk of tool call arguments.
    ArgumentChunk(String),
}

impl EventPart {
    /// Returns the text content of this part, if it's a message or reasoning
    /// chunk.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Message(s) | Self::Reasoning(s) => Some(s),
            Self::Structured(_) | Self::ToolCall(_) => None,
        }
    }

    /// Returns a mutable reference to the text content, if it's a message or
    /// reasoning chunk.
    #[must_use]
    pub fn as_text_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::Message(s) | Self::Reasoning(s) => Some(s),
            Self::Structured(_) | Self::ToolCall(_) => None,
        }
    }

    /// Returns `true` if this is a reasoning chunk.
    #[must_use]
    pub const fn is_reasoning(&self) -> bool {
        matches!(self, Self::Reasoning(_))
    }

    /// Returns `true` if this is a tool call event.
    #[must_use]
    pub const fn is_tool_call(&self) -> bool {
        matches!(self, Self::ToolCall(_))
    }
}

/// A patch to apply to historical conversation events.
///
/// Providers yield these (via [`Event::Patch`]) when they discover that
/// previously-persisted events need updating.
#[derive(Debug, Clone, PartialEq)]
pub struct EventPatch {
    /// Which events to target.
    pub matcher: EventMatcher,

    /// What to do with matching events.
    pub action: PatchAction,
}

/// Identifies which historical events a patch applies to.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum EventMatcher {
    /// Match events where `metadata[key]` equals `value` (string comparison).
    MetadataValue { key: String, value: String },
}

/// The mutation to apply to matched events.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum PatchAction {
    /// Remove a metadata key from the event.
    RemoveMetadata(String),
}

impl PatchAction {
    /// Whether applying this action strictly shrinks the conversation stream.
    ///
    /// A provider-driven repair loop rebuilds the request after every applied
    /// patch, so it terminates only if each round shrinks a finite measure.
    /// Producing a change is not enough: an action that sets a fresh value or
    /// appends content can change an event on every round forever.
    ///
    /// An action that cannot guarantee shrinkage returns `false`, and the
    /// repair loop refuses to rebuild rather than risk an unbounded number of
    /// requests.
    /// Answer this deliberately when adding a variant; `false` is the safe
    /// answer.
    #[must_use]
    pub const fn shrinks_stream(&self) -> bool {
        match self {
            Self::RemoveMetadata(_) => true,
        }
    }
}

/// Record provider-issued patches against `stream`, returning how many events
/// they change in the projected stream.
///
/// The patches are appended as an overlay: the stored events keep their
/// metadata, and the removal happens when the stream is projected to build a
/// request.
/// A repair made for one provider therefore does not destroy metadata another
/// provider or model could still use, and the conversation stays an append-only
/// log.
///
/// A return value of `0` means the projection is unchanged, so resending a
/// request built from it would hit the same provider complaint.
/// That happens when the patches match nothing, when a matched event does not
/// carry the field the action removes, or when an earlier overlay already
/// removed it: matching alone is not progress.
pub fn record_patches(stream: &mut ConversationStream, patches: &[EventPatch]) -> usize {
    stream.add_overlay(patches.iter().map(OverlayPatch::from).collect())
}

impl From<&EventPatch> for OverlayPatch {
    fn from(patch: &EventPatch) -> Self {
        Self {
            matcher: match &patch.matcher {
                EventMatcher::MetadataValue { key, value } => OverlayMatcher::MetadataValue {
                    key: key.clone(),
                    value: value.clone(),
                },
            },
            action: match &patch.action {
                PatchAction::RemoveMetadata(key) => {
                    OverlayAction::RemoveMetadata { key: key.clone() }
                }
            },
        }
    }
}

impl Event {
    /// Create a new [`Event::Part`] with a message chunk.
    #[must_use]
    pub fn message(index: usize, content: impl Into<String>) -> Self {
        Self::Part {
            index,
            part: EventPart::Message(content.into()),
            metadata: Map::new(),
        }
    }

    /// Create a new [`Event::Part`] with a reasoning chunk.
    #[must_use]
    pub fn reasoning(index: usize, content: impl Into<String>) -> Self {
        Self::Part {
            index,
            part: EventPart::Reasoning(content.into()),
            metadata: Map::new(),
        }
    }

    /// Create a new [`Event::Part`] with a structured JSON chunk.
    #[must_use]
    pub fn structured(index: usize, content: impl Into<String>) -> Self {
        Self::Part {
            index,
            part: EventPart::Structured(content.into()),
            metadata: Map::new(),
        }
    }

    /// Create a new [`Event::Part`] with a tool call start signal.
    #[must_use]
    pub fn tool_call_start(index: usize, id: impl Into<String>, name: impl Into<String>) -> Self {
        Self::Part {
            index,
            part: EventPart::ToolCall(ToolCallPart::Start {
                id: id.into(),
                name: name.into(),
            }),
            metadata: Map::new(),
        }
    }

    /// Create a new [`Event::Part`] with a tool call argument chunk.
    #[must_use]
    pub fn tool_call_args(index: usize, json: impl Into<String>) -> Self {
        Self::Part {
            index,
            part: EventPart::ToolCall(ToolCallPart::ArgumentChunk(json.into())),
            metadata: Map::new(),
        }
    }

    /// Create a new [`Event::Flush`] event.
    #[must_use]
    pub fn flush(index: usize) -> Self {
        Self::flush_with_metadata(index, Map::new())
    }

    /// Create a new [`Event::Flush`] event with a single metadata field.
    #[must_use]
    pub fn flush_with_metadata_field(
        index: usize,
        key: impl Into<String>,
        value: impl Into<Value>,
    ) -> Self {
        Self::flush_with_metadata(index, Map::from_iter([(key.into(), value.into())]))
    }

    /// Create a new [`Event::Flush`] event with additional metadata.
    #[must_use]
    pub fn flush_with_metadata(index: usize, metadata: Map<String, Value>) -> Self {
        Self::Flush { index, metadata }
    }

    /// Returns the [`EventPart`] if this is a `Part` event.
    #[must_use]
    pub fn as_part(&self) -> Option<&EventPart> {
        match self {
            Self::Part { part, .. } => Some(part),
            _ => None,
        }
    }

    /// Returns a mutable reference to the [`EventPart`] if this is a `Part`
    /// event.
    #[must_use]
    pub fn as_part_mut(&mut self) -> Option<&mut EventPart> {
        match self {
            Self::Part { part, .. } => Some(part),
            _ => None,
        }
    }

    /// Attaches a metadata field to a `Part` or `Flush` event.
    /// No-op for other variants.
    #[must_use]
    pub fn with_metadata_field(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.add_metadata_field(key, value);
        self
    }

    /// Attaches a metadata field to a `Part` or `Flush` event.
    /// No-op for other variants.
    pub fn add_metadata_field(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        match self {
            Self::Part { metadata, .. } | Self::Flush { metadata, .. } => {
                metadata.insert(key.into(), value.into());
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FinishReason {
    /// The turn was completed by the assistant.
    Completed,

    /// The maximum number of tokens was reached before the assistant could
    /// complete the turn.
    MaxTokens,

    /// The provider requests that the caller rebuild the request from the
    /// current conversation state and retry.
    /// The stream is finished and no further events will be produced.
    Retry,

    /// A safety classifier declined the request.
    ///
    /// This is a successful, terminal state: the model chose not to answer
    /// rather than failing.
    /// Any partial output already streamed should be discarded.
    Refused {
        /// The policy area that triggered the decline (e.g. `cyber`, `bio`), or
        /// `None` when the refusal maps to no named category.
        category: Option<String>,

        /// A human-readable explanation.
        /// Display it; do not parse it.
        explanation: Option<String>,
    },

    /// The assistant has stopped generating tokens for some reason.
    Other(Value),
}

#[cfg(test)]
#[path = "event_tests.rs"]
mod tests;
