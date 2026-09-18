//! What one entry in a conversation stream is, and how it is stored.
//!
//! An entry is an identity plus a payload.
//! [`InternalEvent`] pairs the two and is what a stream holds; [`EventPayload`]
//! is the payload alone, and is where the on-disk `type` tag and the base64
//! encoding of content fields live.
//!
//! Reading an entry back goes through [`StoredEvent`], whose identity is
//! optional: a file can carry an entry with no ID, or two entries with the same
//! one, and only a stream can settle that.
//! `ConversationStream::from_parts` is what turns a `StoredEvent` into an
//! `InternalEvent`, which is why "unique within its stream" holds of every
//! entry a stream holds.
//!
//! Both serde directions are hand-rolled.
//! Serialization writes `event_id` alongside a flattened payload, so an entry
//! is one JSON object.
//! Deserialization dispatches on the `type` tag rather than trying each variant
//! in turn: `cargo dhat` showed the untagged approach allocating heavily on
//! stream loads.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use serde_json::{Value, from_value};
use tracing::warn;

use super::config_delta::{self, ConfigDelta};
use crate::{
    Compaction, EventId, EventOverlay,
    event::{ConversationEvent, EventKind},
    storage::{decode_event_value, encode_event},
};

/// A stream entry with stream-assigned identity and a flattened storage
/// payload.
///
/// An `InternalEvent` only exists inside a stream, and its `event_id` is one
/// that stream handed out.
/// Reading one from storage goes through [`StoredEvent`], whose ID is optional
/// until the stream settles it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(super) struct InternalEvent {
    /// Identity within the raw conversation stream.
    pub(super) event_id: EventId,
    /// Stored content, including the producer's timestamp.
    #[serde(flatten)]
    pub(super) payload: EventPayload,
}

/// Stored payload with type tagging and base64 encoding for content fields.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum EventPayload {
    /// The configuration state of the conversation is updated.
    ///
    /// When this event is emitted, all subsequent events in the stream are
    /// bound to the new configuration.
    ///
    /// An [`Apply`] delta is merged on top of all previous `ConfigDelta` events
    /// in the stream; a [`Reset`] discards the accumulated state, restarting
    /// from program defaults.
    ///
    /// Any non-config events before the first `ConfigDelta` event are
    /// considered to have the default configuration.
    ///
    /// [`Apply`]: ConfigDelta::Apply
    /// [`Reset`]: ConfigDelta::Reset
    ConfigDelta(ConfigDelta),
    /// An event in the conversation stream.
    Event(Box<ConversationEvent>),
    /// A compaction overlay that modifies how preceding events are projected
    /// when building the LLM request.
    /// Does not modify or delete any existing events.
    Compaction(Compaction),
    /// A patch overlay that rewrites how matched events are projected when
    /// building the LLM request.
    /// Does not modify or delete any existing events.
    Overlay(EventOverlay),
    /// An event whose `type` tag this build does not recognize.
    ///
    /// Conversations are an append-only log that a newer `jp` may have written.
    /// Rather than fail the entire stream load on an unknown event kind, the
    /// raw JSON is retained verbatim so it round-trips losslessly on the next
    /// save.
    /// Unknown events are invisible to event iteration, config resolution, and
    /// providers.
    Unknown(Value),
}

impl Serialize for EventPayload {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::ConfigDelta(delta) => {
                #[derive(Serialize)]
                struct Tagged<'a> {
                    #[serde(rename = "type")]
                    tag: &'static str,
                    #[serde(flatten)]
                    inner: &'a ConfigDelta,
                }

                Tagged {
                    tag: "config_delta",
                    inner: delta,
                }
                .serialize(serializer)
            }
            Self::Event(event) => {
                let mut value =
                    serde_json::to_value(event.as_ref()).map_err(serde::ser::Error::custom)?;

                // Base64-encode storage fields.
                encode_event(&mut value, &event.kind);
                value.serialize(serializer)
            }
            Self::Compaction(compaction) => {
                #[derive(Serialize)]
                struct Tagged<'a> {
                    #[serde(rename = "type")]
                    tag: &'static str,
                    #[serde(flatten)]
                    inner: &'a Compaction,
                }

                Tagged {
                    tag: "compaction",
                    inner: compaction,
                }
                .serialize(serializer)
            }
            Self::Overlay(overlay) => {
                #[derive(Serialize)]
                struct Tagged<'a> {
                    #[serde(rename = "type")]
                    tag: &'static str,
                    #[serde(flatten)]
                    inner: &'a EventOverlay,
                }

                Tagged {
                    tag: "event_overlay",
                    inner: overlay,
                }
                .serialize(serializer)
            }
            Self::Unknown(value) => value.serialize(serializer),
        }
    }
}

// Dispatching on `type` avoids the allocations from trying each untagged
// variant. Base64 content is decoded only for known conversation events.
impl<'de> Deserialize<'de> for EventPayload {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut value = Value::deserialize(deserializer)?;

        let tag = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        if tag == "config_delta" {
            return config_delta::deserialize(&value)
                .map(Self::ConfigDelta)
                .map_err(D::Error::custom);
        }

        if tag == "compaction" {
            return serde_json::from_value(value)
                .map(Self::Compaction)
                .map_err(D::Error::custom);
        }

        if tag == "event_overlay" {
            return serde_json::from_value(value)
                .map(Self::Overlay)
                .map_err(D::Error::custom);
        }

        // Conversations are an append-only log a newer `jp` may have written.
        // An unrecognized event kind is preserved verbatim instead of failing
        // the whole stream load, so it round-trips on the next save. Corrupt
        // *known* events still fail loudly below.
        if !EventKind::TYPE_TAGS.contains(&tag) {
            #[cfg(debug_assertions)]
            {
                let mut probe = value.clone();
                decode_event_value(&mut probe);
                debug_assert!(
                    serde_json::from_value::<ConversationEvent>(probe).is_err(),
                    "event tag `{tag}` is missing from EventKind::TYPE_TAGS",
                );
            }
            warn!(%tag, "Unknown conversation event kind; preserving raw event.");
            return Ok(Self::Unknown(value));
        }

        // Decode base64-encoded storage fields before deserializing.
        decode_event_value(&mut value);

        serde_json::from_value(value)
            .map(|e| Self::Event(Box::new(e)))
            .map_err(D::Error::custom)
    }
}

/// Whether an [`InternalEvent`] belongs to a single turn or applies to the
/// conversation as a whole.
///
/// This is the single source of truth for which events survive turn-level
/// pruning (`pop`, `trim_chat_request`, `pop_if`, `retain`).
/// Adding a new `EventPayload` variant forces a classification here:
/// [`InternalEvent::scope`] is an exhaustive match, so no pruning caller can
/// silently mistreat it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EventScope {
    /// Survives turn pruning: config deltas and compaction overlays apply to
    /// the conversation regardless of position.
    Global,
    /// Belongs to a turn and is removed when that turn is pruned.
    Turn,
}

impl InternalEvent {
    /// Consume the entry, returning its conversation event if it has one.
    #[must_use]
    pub(super) fn into_event(self) -> Option<ConversationEvent> {
        match self.payload {
            EventPayload::Event(event) => Some(*event),
            EventPayload::ConfigDelta(_)
            | EventPayload::Compaction(_)
            | EventPayload::Overlay(_)
            | EventPayload::Unknown(_) => None,
        }
    }

    /// Get a reference to [`EventPayload::Event`], if applicable.
    #[must_use]
    pub(super) fn as_event(&self) -> Option<&ConversationEvent> {
        match &self.payload {
            EventPayload::Event(event) => Some(event),
            EventPayload::ConfigDelta(_)
            | EventPayload::Compaction(_)
            | EventPayload::Overlay(_)
            | EventPayload::Unknown(_) => None,
        }
    }

    /// Classify the event as turn-scoped or global.
    /// See [`EventScope`].
    #[must_use]
    pub(super) const fn scope(&self) -> EventScope {
        match &self.payload {
            EventPayload::ConfigDelta(_)
            | EventPayload::Compaction(_)
            | EventPayload::Overlay(_)
            | EventPayload::Unknown(_) => EventScope::Global,
            EventPayload::Event(_) => EventScope::Turn,
        }
    }
}

/// A stream entry as storage holds it, before a stream settles its identity.
///
/// `event_id` is `Option` because identity belongs to the stream, not to the
/// file: a legacy entry has none, and a hand-edited file can give two entries
/// the same one.
/// Only `ConversationStream::from_parts` turns this into an [`InternalEvent`],
/// which is what keeps "unique within its stream" true of every entry a stream
/// holds rather than of every entry that happens to have been read.
#[derive(Debug)]
pub(super) struct StoredEvent {
    /// The ID the file carried, if any.
    pub(super) event_id: Option<EventId>,

    /// The entry's content.
    pub(super) payload: EventPayload,
}

impl StoredEvent {
    /// Settle this entry's identity on its own, outside a stream.
    ///
    /// A test asserting on one entry's storage round-trip has no stream to take
    /// an ID from, and reading a single entry cannot check uniqueness against
    /// entries it never saw.
    /// Loading a stream goes through `ConversationStream::from_parts`, which
    /// settles every entry's ID together.
    #[cfg(test)]
    pub(super) fn into_entry(self) -> InternalEvent {
        InternalEvent {
            event_id: self.event_id.unwrap_or_else(EventId::random),
            payload: self.payload,
        }
    }
}

// `event_id` is lifted out of the stored object before the payload is read, so
// an `Unknown` entry retains its raw JSON without a second copy of the key and
// writes exactly one back out. `shift_remove` keeps the remaining keys in their
// stored order, so a hand-edited entry round-trips unreordered.
//
// A missing, null, or empty `event_id` reads as absent, which is leniency the
// storage boundary owes a legacy file; `EventId` itself stays strict. Any other
// non-string `event_id` is a corrupt entry rather than an absent one, and fails
// loudly.
impl<'de> Deserialize<'de> for StoredEvent {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut value = Value::deserialize(deserializer)?;
        let object = value
            .as_object_mut()
            .ok_or_else(|| D::Error::custom("stream entry must be a JSON object"))?;
        let event_id = match object.shift_remove("event_id") {
            None | Some(Value::Null) => None,
            Some(Value::String(id)) if id.is_empty() => None,
            Some(id) => Some(from_value(id).map_err(D::Error::custom)?),
        };
        let payload = from_value(value).map_err(D::Error::custom)?;
        Ok(Self { event_id, payload })
    }
}
