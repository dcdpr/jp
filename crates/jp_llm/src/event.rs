use indexmap::IndexMap;
use jp_conversation::ConversationEvent;
use serde_json::Value;

/// Represents a completed event from the LLM.
///
/// In the context of [`Provider::chat_completion_stream`], individual
/// [`Event::Part`]s represent incomplete chunks of data streamed from the
/// provider.
///
/// These parts will have their `index` field set to the index of the final
/// messages array streamed by the provider. For example, if the LLM produces
/// reasoning tokens, response tokens and tool call tokens, then all of these
/// will have different `index` values corresponding to their relative order.
///
/// Only after [`Event::Flush`] is produced with the given `index` value, should
/// all previous parts be merged into a single [`ConversationEvent`], using
/// [`EventAggregator`].
///
/// For [`Provider::chat_completion`], the same applies, but since this is a
/// non-streaming API, every [`Event::Part`] will be followed by an
/// [`Event::Flush`] event.
#[derive(Debug, Clone)]
pub enum Event {
    /// A part of a completed event.
    Part {
        /// The index of the final message in the array of complete messages
        /// streamed by the provider.
        ///
        /// For example, if the LLM produces reasoning tokens, response tokens
        /// and tool call tokens, then the chunks for all of these will have
        /// different `index` values corresponding to their relative order.
        index: usize,

        /// The event.
        event: ConversationEvent,
    },

    /// Flush one or more [`Event::Part`]s.
    ///
    /// Can optionally carry final metadata (e.g. usage, signatures) to attach
    /// to the event before emitting it.
    Flush {
        /// The index of the [`Event::Part`]s to flush.
        index: usize,

        /// Additional opaque metadata associated with the event.
        metadata: IndexMap<String, Value>,
    },

    /// The response was finished.
    Finished(FinishReason),
}

impl Event {
    /// Create a new [`Event::Flush`] event.
    #[must_use]
    pub fn flush(index: usize) -> Self {
        Self::flush_with_metadata(index, IndexMap::new())
    }

    /// Create a new [`Event::Flush`] event with a single metadata field.
    #[must_use]
    pub fn flush_with_metadata_field(
        index: usize,
        key: impl Into<String>,
        value: impl Into<Value>,
    ) -> Self {
        Self::flush_with_metadata(index, IndexMap::from_iter([(key.into(), value.into())]))
    }

    /// Create a new [`Event::Flush`] event with additional metadata.
    #[must_use]
    pub fn flush_with_metadata(index: usize, metadata: IndexMap<String, Value>) -> Self {
        Self::Flush { index, metadata }
    }

    #[must_use]
    pub fn is_conversation_event(&self) -> bool {
        matches!(self, Self::Part { .. })
    }

    #[must_use]
    pub fn as_conversation_event(&self) -> Option<&ConversationEvent> {
        match self {
            Self::Part { event, .. } => Some(event),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_conversation_event_mut(&mut self) -> Option<&mut ConversationEvent> {
        match self {
            Self::Part { event, .. } => Some(event),
            _ => None,
        }
    }

    #[must_use]
    pub fn into_conversation_event(self) -> Option<ConversationEvent> {
        match self {
            Self::Part { event, .. } => Some(event),
            _ => None,
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

    /// The assistant has stopped generating tokens for some reason.
    Other(Value),
}
