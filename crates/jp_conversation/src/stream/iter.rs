//! Walking a conversation stream, and what one step of that walk yields.
//!
//! Every iterator here resolves the configuration each event was recorded
//! under, and yields a `…WithConfig` view carrying it.
//! A stream stores config as a base plus a series of deltas, so an event's
//! config is the base with every delta *before it* folded in: a forward walk
//! carries that state as it goes, and a reverse walk reconstructs it.
//!
//! Entries that are not conversation events are stepped over: a config delta is
//! folded into the running state, and compactions, patch overlays, and entries
//! this build does not recognize are skipped entirely.
//!
//! Three iterators, by what the caller may do with what it gets:
//!
//! - [`Iter`] borrows, and walks from either end.
//! - [`IterMut`] borrows mutably, forwards only.
//! - [`IntoIter`] takes ownership, and walks from either end.
//!
//! Each yields an event's entry ID alongside it.
//! Editing an event through [`IterMut`] does not change its ID: identity
//! belongs to the entry, not to its content.

use jp_config::{PartialAppConfig, PartialConfig as _};
use tracing::error;

use super::{
    ConversationStream, StreamError,
    config_delta::{self, ApplyDelta},
    entry::{EventPayload, InternalEvent},
};
use crate::{ConversationEvent, EventId, EventKind};

/// Append events carrying their own identity and config state.
///
/// Each event keeps its [`EventId`] unless this stream has already handed that
/// ID out, so moving events between streams preserves references into the
/// source.
///
/// The config deltas separating them are not copied: they are recomputed
/// against this stream's running config state, which is what suits a
/// destination whose base config differs from the source's.
/// A recomputed delta is a new entry and is assigned a new ID.
impl Extend<ConversationEventWithConfig> for ConversationStream {
    fn extend<T: IntoIterator<Item = ConversationEventWithConfig>>(&mut self, iter: T) {
        // Cache the running tail config across iterations. Without this, every
        // push falls through `push_with_config_delta` → `self.last()`, which
        // walks the whole stream and deep-clones `PartialAppConfig` on each
        // step — making `extend(n)` O(n²) in clones.
        let mut tail = self
            .last()
            .map_or_else(|| self.base_config().to_partial(), |v| v.config);

        for v in iter {
            let ConversationEventWithConfig {
                event_id,
                event,
                config,
            } = v;
            let config_delta = tail.delta(config.clone());

            if !config_delta.is_empty() {
                self.add_config_delta(ApplyDelta::new(event.timestamp, config_delta));
            }

            tail = config;
            self.adopt(InternalEvent {
                event_id,
                payload: EventPayload::Event(Box::new(event)),
            });
        }
    }
}

/// Append bare events, each receiving an ID assigned by this stream.
///
/// A [`ConversationEvent`] carries no identity of its own; identity belongs to
/// the stream entry wrapping it.
impl Extend<ConversationEvent> for ConversationStream {
    fn extend<T: IntoIterator<Item = ConversationEvent>>(&mut self, iter: T) {
        for v in iter {
            self.push_event(v);
        }
    }
}

impl IntoIterator for ConversationStream {
    type IntoIter = IntoIter;
    type Item = ConversationEventWithConfig;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            current_config: self.base_config().to_partial(),
            inner_iter: self.events.into_iter(),
        }
    }
}

/// An owned iterator over the events in a conversation stream.
pub struct IntoIter {
    /// The configuration state for the next event in the iterator.
    current_config: PartialAppConfig,

    /// The iterator over the events in the stream.
    inner_iter: std::vec::IntoIter<InternalEvent>,
}

impl Iterator for IntoIter {
    type Item = ConversationEventWithConfig;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let InternalEvent { event_id, payload } = self.inner_iter.next()?;

            match payload {
                EventPayload::ConfigDelta(delta) => {
                    if let Err(error) = config_delta::fold(&mut self.current_config, delta) {
                        error!(%error, "Failed to merge config delta.");
                    }
                }
                EventPayload::Event(event) => {
                    return Some(ConversationEventWithConfig {
                        event_id,
                        event: *event,
                        config: self.current_config.clone(),
                    });
                }
                EventPayload::Compaction(_)
                | EventPayload::Overlay(_)
                | EventPayload::Unknown(_) => {}
            }
        }
    }
}

impl DoubleEndedIterator for IntoIter {
    fn next_back(&mut self) -> Option<Self::Item> {
        loop {
            let InternalEvent { event_id, payload } = self.inner_iter.next_back()?;

            match payload {
                EventPayload::ConfigDelta(_)
                | EventPayload::Compaction(_)
                | EventPayload::Overlay(_)
                | EventPayload::Unknown(_) => {
                    // A delta/compaction at the very end of the list affects
                    // nothing that follows it, and it doesn't affect previous
                    // items. We simply discard it.
                    // event at the tail likewise yields no ConversationEvent.
                }
                EventPayload::Event(event) => {
                    // Start with the state currently at the front of the line
                    let mut config = self.current_config.clone();

                    // Scan the remaining items in the middle (without consuming
                    // them) to apply all pending deltas to our temporary
                    // config.
                    for internal_event in self.inner_iter.as_slice() {
                        if let EventPayload::ConfigDelta(delta) = &internal_event.payload
                            && let Err(error) = config_delta::fold(&mut config, delta.clone())
                        {
                            error!(%error, "Failed to merge config delta.");
                        }
                    }

                    return Some(ConversationEventWithConfig {
                        event_id,
                        event: *event,
                        config,
                    });
                }
            }
        }
    }
}

/// An iterator over the borrowed events in a conversation stream.
pub(super) struct Iter<'a> {
    /// The stream being iterated over.
    pub(super) stream: &'a ConversationStream,

    /// The configuration state for the first, next event in the iterator.
    pub(super) front_config: PartialAppConfig,

    /// The index of the `next` event in the iterator.
    pub(super) front: usize,

    /// The index of the `next_back` event in the iterator.
    pub(super) back: usize,
}

impl<'a> Iterator for Iter<'a> {
    type Item = ConversationEventWithConfigRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.front < self.back {
            let internal = &self.stream.events[self.front];
            let event_id = &internal.event_id;
            self.front += 1;

            match &internal.payload {
                EventPayload::ConfigDelta(delta) => {
                    if let Err(error) = config_delta::fold(&mut self.front_config, delta.clone()) {
                        error!(%error, "Failed to merge config delta.");
                    }
                }
                EventPayload::Event(event) => {
                    return Some(ConversationEventWithConfigRef {
                        event_id,
                        event,
                        config: self.front_config.clone(),
                    });
                }
                EventPayload::Compaction(_)
                | EventPayload::Overlay(_)
                | EventPayload::Unknown(_) => {}
            }
        }

        None
    }
}

impl DoubleEndedIterator for Iter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        while self.back > self.front {
            self.back -= 1;
            let internal = &self.stream.events[self.back];
            let event_id = &internal.event_id;

            let EventPayload::Event(event) = &internal.payload else {
                continue;
            };

            let mut config = self.stream.base_config().to_partial();
            for internal_event in &self.stream.events[..self.back] {
                if let EventPayload::ConfigDelta(delta) = &internal_event.payload
                    && let Err(error) = config_delta::fold(&mut config, delta.clone())
                {
                    error!(%error, "Failed to merge config delta.");
                }
            }

            return Some(ConversationEventWithConfigRef {
                event_id,
                event,
                config,
            });
        }

        None
    }
}

/// An iterator over the mutable events in a conversation stream.
pub struct IterMut<'a> {
    /// The configuration state for the first, next event in the iterator.
    pub(super) front_config: PartialAppConfig,

    /// The iterator over the events in the stream.
    pub(super) iter: std::slice::IterMut<'a, InternalEvent>,
}

impl<'a> Iterator for IterMut<'a> {
    type Item = ConversationEventWithConfigMut<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        for internal in self.iter.by_ref() {
            let event_id = &internal.event_id;
            match &mut internal.payload {
                EventPayload::ConfigDelta(delta) => {
                    if let Err(error) = config_delta::fold(&mut self.front_config, delta.clone()) {
                        error!(%error, "Failed to merge config delta.");
                    }
                }
                EventPayload::Event(event) => {
                    return Some(ConversationEventWithConfigMut {
                        event_id,
                        event,
                        config: self.front_config.clone(),
                    });
                }
                EventPayload::Compaction(_)
                | EventPayload::Overlay(_)
                | EventPayload::Unknown(_) => {}
            }
        }

        None
    }
}

/// A [`ConversationEvent`] with the turn it belongs to and its entry ID.
///
/// Yielded by [`ConversationStream::iter_events_by_turn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventInTurn<'a> {
    /// 0-based index of the turn holding this event.
    pub turn: usize,

    /// The identity of this entry in its stream.
    pub event_id: &'a EventId,

    /// The event.
    pub event: &'a ConversationEvent,
}

impl std::ops::Deref for EventInTurn<'_> {
    type Target = ConversationEvent;

    fn deref(&self) -> &Self::Target {
        self.event
    }
}

/// A reference to a [`ConversationEvent`] with its configuration.
#[derive(Debug, PartialEq, Clone)]
pub struct ConversationEventWithConfigRef<'a> {
    /// The identity of this entry in its stream.
    pub event_id: &'a EventId,

    /// The event.
    pub event: &'a ConversationEvent,

    /// The configuration.
    pub config: PartialAppConfig,
}

/// A mutable reference to a [`ConversationEvent`] with its configuration.
#[derive(Debug, PartialEq)]
pub struct ConversationEventWithConfigMut<'a> {
    /// The identity of this entry, unchanged by payload edits.
    pub event_id: &'a EventId,

    /// The event.
    pub event: &'a mut ConversationEvent,

    /// The configuration.
    pub config: PartialAppConfig,
}

/// A [`ConversationEvent`] with its configuration.
#[derive(Debug, PartialEq, Clone)]
pub struct ConversationEventWithConfig {
    /// The identity of this entry in its source stream.
    pub event_id: EventId,

    /// The event.
    pub event: ConversationEvent,

    /// The configuration at the time the event was added.
    ///
    /// It should be noted that this is not necessarily the same as the current
    /// active configuration of the application, even if this is the latest
    /// event in the stream.
    /// For one, the event may have been added a while ago, but more
    /// importantly, not all configuration changes are automatically applied to
    /// a [`ConversationStream`].
    /// For example, if a new tool is added in the configuration, it will not
    /// become available in the conversation stream until explicitly added using
    /// the CLI flag `--tool` or `--cfg`, while *NEW* conversations *WILL* get
    /// the new tool by default.
    pub config: PartialAppConfig,
}

impl ConversationEventWithConfig {
    /// Consume the type and return the underlying [`ConversationEvent`].
    #[must_use]
    pub fn into_inner(self) -> ConversationEvent {
        self.event
    }

    /// Consume the type and return the underlying [`EventKind`].
    #[must_use]
    pub fn into_kind(self) -> EventKind {
        self.event.kind
    }

    /// Return a reference to the underlying [`EventKind`].
    #[must_use]
    pub const fn kind(&self) -> &EventKind {
        &self.event.kind
    }
}

impl From<ConversationEventWithConfigRef<'_>> for ConversationEventWithConfig {
    fn from(value: ConversationEventWithConfigRef<'_>) -> Self {
        Self {
            event_id: value.event_id.clone(),
            event: value.event.clone(),
            config: value.config,
        }
    }
}

impl FromIterator<ConversationEventWithConfig> for Result<ConversationStream, StreamError> {
    fn from_iter<T: IntoIterator<Item = ConversationEventWithConfig>>(iter: T) -> Self {
        let mut events = iter.into_iter();

        let Some((config, first_id, first_event)) =
            events.next().map(|e| (e.config, e.event_id, e.event))
        else {
            return Err(StreamError::FromEmptyIterator);
        };

        let mut stream = ConversationStream::new(jp_config::util::build(config)?.into());
        stream.adopt(InternalEvent {
            event_id: first_id,
            payload: EventPayload::Event(Box::new(first_event)),
        });
        stream.extend(events);

        Ok(stream)
    }
}

impl std::ops::Deref for ConversationEventWithConfig {
    type Target = ConversationEvent;

    fn deref(&self) -> &Self::Target {
        &self.event
    }
}

impl std::ops::Deref for ConversationEventWithConfigRef<'_> {
    type Target = ConversationEvent;

    fn deref(&self) -> &Self::Target {
        self.event
    }
}

impl std::ops::Deref for ConversationEventWithConfigMut<'_> {
    type Target = ConversationEvent;

    fn deref(&self) -> &Self::Target {
        self.event
    }
}

impl std::ops::DerefMut for ConversationEventWithConfigMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.event
    }
}
