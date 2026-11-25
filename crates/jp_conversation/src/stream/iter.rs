//! Config-resolving iterators and item types for [`ConversationStream`].
//!
//! [`ConversationStream::iter_with_config`] (borrowed, [`Iter`]) and
//! [`ConversationStream::into_iter`] (owned, [`IntoIter`]) walk the stream from
//! either end, yielding each [`ConversationEvent`] with its [`EventId`] and the
//! [`PartialAppConfig`] in effect when it was recorded.
//! For event-only iteration without config resolution, use
//! [`ConversationStream::iter`] or [`ConversationStream::iter_mut`].
//!
//! A stream stores a base config plus inline [`ConfigDelta`] entries.
//! Forward iteration folds deltas into the running config as it advances;
//! backward iteration precomputes the config after each remaining delta on the
//! first [`next_back`] call and pops them as it steps back over deltas.
//! Consecutive events with no config delta between them share a single
//! `Arc<PartialAppConfig>` in both directions.
//!
//! Non-event entries (config deltas, compactions, overlays, and unrecognized
//! entries) are not yielded.
//!
//! [`ConfigDelta`]: super::ConfigDelta
//! [`next_back`]: DoubleEndedIterator::next_back

use std::sync::Arc;

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
        // Cache the running tail config across iterations. Recomputing it with
        // `self.last()` per event folds every config delta in the stream each
        // time, which makes `extend(n)` quadratic.
        let mut tail = self
            .last()
            .map_or_else(|| Arc::new(self.base_config().to_partial()), |v| v.config);

        for v in iter {
            let ConversationEventWithConfig {
                event_id,
                event,
                config,
            } = v;

            // Consecutive events with no config delta between them share the
            // same `Arc`, so skip diffing when the pointer has not changed.
            if !Arc::ptr_eq(&tail, &config) {
                let config_delta = tail.delta((*config).clone());
                if !config_delta.is_empty() {
                    self.add_config_delta(ApplyDelta::new(event.timestamp, config_delta));
                }
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
            current_config: Arc::new(self.base_config().to_partial()),
            back_configs: None,
            inner_iter: self.events.into_iter(),
        }
    }
}

/// An owned iterator over the events and configs in a [`ConversationStream`].
pub struct IntoIter {
    /// Running config for forward iteration, shared across events until the
    /// next config delta.
    current_config: Arc<PartialAppConfig>,

    /// Precomputed configs for backward iteration, initialized on the first
    /// `next_back` call.
    back_configs: Option<BackConfigs>,

    /// Remaining stream entries to iterate over.
    inner_iter: std::vec::IntoIter<InternalEvent>,
}

impl Iterator for IntoIter {
    type Item = ConversationEventWithConfig;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let InternalEvent { event_id, payload } = self.inner_iter.next()?;

            match payload {
                EventPayload::ConfigDelta(delta) => {
                    let config = Arc::make_mut(&mut self.current_config);
                    if let Err(error) = config_delta::fold(config, delta) {
                        error!(%error, "Failed to merge config delta.");
                    }
                }
                EventPayload::Event(event) => {
                    return Some(ConversationEventWithConfig {
                        event_id,
                        event: *event,
                        config: Arc::clone(&self.current_config),
                    });
                }
                EventPayload::Compaction(_)
                | EventPayload::Overlay(_)
                | EventPayload::Unknown { .. } => {}
            }
        }
    }
}

impl DoubleEndedIterator for IntoIter {
    fn next_back(&mut self) -> Option<Self::Item> {
        let back = self.back_configs.get_or_insert_with(|| {
            BackConfigs::new(&self.current_config, self.inner_iter.as_slice())
        });

        loop {
            let InternalEvent { event_id, payload } = self.inner_iter.next_back()?;

            match payload {
                EventPayload::ConfigDelta(_) => back.step_over_delta(),
                EventPayload::Event(event) => {
                    return Some(ConversationEventWithConfig {
                        event_id,
                        event: *event,
                        config: back.current(),
                    });
                }
                EventPayload::Compaction(_)
                | EventPayload::Overlay(_)
                | EventPayload::Unknown { .. } => {}
            }
        }
    }
}

/// Precomputed config snapshots for backward iteration.
///
/// Built once on the first `next_back` call from `front` (the config in effect
/// before the remaining slice) and the config after each remaining
/// [`ConfigDelta`] in stream order.
/// Advancing the front of the iterator afterwards does not invalidate these
/// snapshots, because an event's config depends only on the deltas that precede
/// it.
///
/// [`ConfigDelta`]: super::ConfigDelta
pub(super) struct BackConfigs {
    /// Config in effect before the first remaining delta.
    front: Arc<PartialAppConfig>,

    /// Config after each remaining delta, in stream order.
    after_deltas: Vec<Arc<PartialAppConfig>>,
}

impl BackConfigs {
    /// Precompute the config after each [`ConfigDelta`] in `entries`, starting
    /// from `front`.
    ///
    /// [`ConfigDelta`]: super::ConfigDelta
    fn new(front: &Arc<PartialAppConfig>, entries: &[InternalEvent]) -> Self {
        let mut after_deltas: Vec<Arc<PartialAppConfig>> = Vec::new();
        for entry in entries {
            let EventPayload::ConfigDelta(delta) = &entry.payload else {
                continue;
            };

            let mut config = (**after_deltas.last().unwrap_or(front)).clone();
            if let Err(error) = config_delta::fold(&mut config, delta.clone()) {
                error!(%error, "Failed to merge config delta.");
            }
            // Push even if folding fails so `after_deltas` stays 1:1 with
            // `ConfigDelta` entries when popping in `step_over_delta`.
            after_deltas.push(Arc::new(config));
        }

        Self {
            front: Arc::clone(front),
            after_deltas,
        }
    }

    /// Return the config in effect at the current back position.
    fn current(&self) -> Arc<PartialAppConfig> {
        Arc::clone(self.after_deltas.last().unwrap_or(&self.front))
    }

    /// Pop the latest delta's config when stepping backward over a
    /// [`ConfigDelta`].
    ///
    /// [`ConfigDelta`]: super::ConfigDelta
    fn step_over_delta(&mut self) {
        self.after_deltas.pop();
    }
}

/// A borrowed iterator over the events and configs in a [`ConversationStream`].
pub(super) struct Iter<'a> {
    /// The stream being iterated over.
    pub(super) stream: &'a ConversationStream,

    /// Running config for forward iteration, shared across events until the
    /// next config delta.
    pub(super) front_config: Arc<PartialAppConfig>,

    /// Precomputed configs for backward iteration, initialized on the first
    /// `next_back` call.
    pub(super) back_configs: Option<BackConfigs>,

    /// Start of the unyielded slice (`stream.events[front..back]`).
    pub(super) front: usize,

    /// End of the unyielded slice (`stream.events[front..back]`).
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
                    let config = Arc::make_mut(&mut self.front_config);
                    if let Err(error) = config_delta::fold(config, delta.clone()) {
                        error!(%error, "Failed to merge config delta.");
                    }
                }
                EventPayload::Event(event) => {
                    return Some(ConversationEventWithConfigRef {
                        event_id,
                        event,
                        config: Arc::clone(&self.front_config),
                    });
                }
                EventPayload::Compaction(_)
                | EventPayload::Overlay(_)
                | EventPayload::Unknown { .. } => {}
            }
        }

        None
    }
}

impl DoubleEndedIterator for Iter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let entries = &self.stream.events[self.front..self.back];
        let back = self
            .back_configs
            .get_or_insert_with(|| BackConfigs::new(&self.front_config, entries));

        while self.back > self.front {
            self.back -= 1;
            let internal = &self.stream.events[self.back];
            let event_id = &internal.event_id;

            match &internal.payload {
                EventPayload::ConfigDelta(_) => back.step_over_delta(),
                EventPayload::Event(event) => {
                    return Some(ConversationEventWithConfigRef {
                        event_id,
                        event,
                        config: back.current(),
                    });
                }
                EventPayload::Compaction(_)
                | EventPayload::Overlay(_)
                | EventPayload::Unknown { .. } => {}
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

    /// The configuration at the time the event was added.
    ///
    /// Events with no config delta between them share one allocation.
    pub config: Arc<PartialAppConfig>,
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
    ///
    /// Events with no config delta between them share one allocation.
    pub config: Arc<PartialAppConfig>,
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

        let config = jp_config::util::build(Arc::unwrap_or_clone(config))?;
        let mut stream = ConversationStream::new(config.into());
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
