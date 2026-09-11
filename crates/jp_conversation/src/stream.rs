//! A conversation's event log.
//!
//! [`ConversationStream`] is the entry point: an ordered list of entries plus
//! the base configuration they layer onto.
//! It owns appending, pruning, repairing, and iterating, and is the only thing
//! that hands out entry IDs.
//!
//! The surrounding modules hold the pieces it is built from:
//!
//! - [`entry`] — what one entry is, and how it is stored.
//! - [`config_delta`] — the entries that change a conversation's config.
//! - [`iter`] — walking the stream, resolving each event's config as it goes.
//! - [`projection`] — the provider-facing view, with overlays applied.
//! - [`turn_iter`] and [`turn_mut`] — reading and writing a turn at a time.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use jp_config::{AppConfig, FillDefaults as _, PartialAppConfig, PartialConfig as _};
use serde_json::{Map, Value};
use tracing::warn;

pub mod config_delta;
mod entry;
pub mod iter;
mod projection;
pub mod turn_iter;
pub mod turn_mut;
pub use config_delta::{ApplyDelta, ConfigDelta, ResetDelta};
pub use iter::{
    ConversationEventWithConfig, ConversationEventWithConfigMut, ConversationEventWithConfigRef,
    EventInTurn, IntoIter, IterMut,
};
pub use projection::{AffectedItem, TurnOrigin};
pub use turn_iter::{IterTurns, Turn};
pub use turn_mut::TurnMut;

use crate::{
    Compaction, EventId, EventOverlay, OverlayPatch,
    event::{ChatRequest, ConversationEvent, EventKind, InquiryId, ToolCallResponse, TurnStart},
    event_id::EventIds,
    stream::{
        entry::{EventPayload, EventScope, InternalEvent, StoredEvent},
        iter::Iter,
    },
};

/// A stream of events that make up a conversation.
#[derive(Debug, Clone)]
pub struct ConversationStream {
    /// The base configuration for the conversation.
    ///
    /// This is the configuration that is used when the conversation is first
    /// created, and is used as the default configuration for all events in the
    /// stream, until a config delta is encountered to amend it.
    ///
    /// This is stored separately from the events in the stream, to guarantee a
    /// stream always has a base configuration.
    base_config: Arc<AppConfig>,

    /// The events in the stream.
    events: Vec<InternalEvent>,

    /// Every entry ID this stream has handed out, including those whose entry
    /// has since been removed.
    ///
    /// A superset of the IDs in `events`, and the invariant every insertion
    /// path relies on: an ID is retired with its entry rather than returned to
    /// circulation, so a reference to a deleted entry fails to resolve instead
    /// of binding to a later one.
    ///
    /// An entry carrying an ID this set has not seen reaches `events` only
    /// through [`Self::append`], [`Self::insert`], or [`Self::adopt`], which is
    /// what keeps the two in step.
    /// Moving an entry the stream already holds does not go through them, and
    /// must not: those take an ID the set has not handed out, so passing one it
    /// has would replace the entry's ID rather than preserve it.
    event_ids: EventIds,

    /// IDs more than one entry carried when this stream was loaded.
    ///
    /// Read through [`Self::duplicated_event_ids`], which states the deadline a
    /// consumer is held to.
    /// Not serialized.
    duplicated_event_ids: HashSet<EventId>,

    /// The timestamp of the creation of the stream.
    pub created_at: DateTime<Utc>,
}

// Hand-rolled to compare what the stream *holds*, not how it came to hold it.
// `event_ids` and `duplicated_event_ids` are load- and history-scoped: two
// streams carrying identical entries would otherwise compare unequal because
// one of them was loaded from a file with duplicate IDs, or because an entry
// was pushed and popped along the way.
impl PartialEq for ConversationStream {
    fn eq(&self, other: &Self) -> bool {
        self.base_config == other.base_config
            && self.events == other.events
            && self.created_at == other.created_at
    }
}

impl ConversationStream {
    /// Create a new [`ConversationStream`] with the given base configuration.
    #[must_use]
    pub fn new(base_config: Arc<AppConfig>) -> Self {
        Self {
            base_config,
            events: Vec::new(),
            event_ids: EventIds::default(),
            duplicated_event_ids: HashSet::new(),
            created_at: Utc::now(),
        }
    }

    /// Set the base configuration for the stream.
    #[must_use]
    pub fn with_base_config(mut self, base_config: Arc<AppConfig>) -> Self {
        self.base_config = base_config;
        self
    }

    /// Set the timestamp of the creation of the stream.
    #[must_use]
    pub fn with_created_at(mut self, created_at: impl Into<DateTime<Utc>>) -> Self {
        self.created_at = created_at.into();
        self
    }

    /// Returns `true` if the stream holds no [`ConversationEvent`]s.
    ///
    /// Entries of other kinds are not counted, so a stream carrying only config
    /// deltas and compactions reports `true` here and still writes those
    /// entries in [`Self::to_parts`].
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self
            .events
            .iter()
            .any(|e| matches!(&e.payload, EventPayload::Event(_)))
    }

    /// Returns `true` if the stream contains at least one [`ChatRequest`].
    #[must_use]
    pub fn has_chat_request(&self) -> bool {
        self.events
            .iter()
            .any(|e| matches!(&e.payload, EventPayload::Event(event) if event.is_chat_request()))
    }

    /// Returns the number of [`ConversationEvent`]s in the stream.
    ///
    /// Entries of other kinds are not counted, so this is at most the number of
    /// entries [`Self::to_parts`] writes, and can be `0` for a stream that
    /// stores several.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events
            .iter()
            .filter(|e| matches!(&e.payload, EventPayload::Event(_)))
            .count()
    }

    /// Return the base configuration for the stream.
    #[must_use]
    pub fn base_config(&self) -> Arc<AppConfig> {
        self.base_config.clone()
    }

    /// Get the merged configuration of the stream.
    ///
    /// This takes the base configuration, and merges all `ConfigDelta` events
    /// in the stream from first to last, including any delta's that come
    /// *after* the last conversation event.
    ///
    /// A [`Reset`] delta discards everything accumulated before it, including
    /// the base configuration's contribution; resolution restarts from program
    /// defaults.
    ///
    /// If you need the configuration state of the last event in the stream, use
    /// [`ConversationStream::last`], which returns a
    /// [`ConversationEventWithConfig`]. containing the `config` field for that
    /// event.
    ///
    /// # Errors
    ///
    /// Returns an error if the merged configuration is invalid.
    ///
    /// [`Reset`]: ConfigDelta::Reset
    pub fn config(&self) -> Result<AppConfig, StreamError> {
        // `build`, not the bare conversion: a delta can introduce a model alias
        // that the base config never had, and reading an unresolved alias panics.
        // `build` is also what orders instructions and prompt sections, which the
        // rest of the system assumes has happened.
        jp_config::util::build(self.config_partial()?).map_err(Into::into)
    }

    /// Get the accumulated config state of the stream, before resolution.
    ///
    /// Takes the base configuration and folds every [`ConfigDelta`] in the
    /// stream onto it, first to last, the same way [`Self::config`] does.
    ///
    /// A field's merge metadata — its strategy, separator, or dedup mode —
    /// lives here and has no counterpart in a resolved [`AppConfig`], which
    /// holds values alone.
    /// A caller merging a further layer onto the conversation's state therefore
    /// starts from this, so the merge runs under the metadata the conversation
    /// established rather than under program defaults.
    ///
    /// # Errors
    ///
    /// Returns an error if a delta cannot be folded onto the accumulated state.
    pub fn config_partial(&self) -> Result<PartialAppConfig, StreamError> {
        let mut partial = self.base_config.to_partial();
        let iter = self.events.iter().filter_map(|event| match &event.payload {
            EventPayload::ConfigDelta(delta) => Some(delta.clone()),
            EventPayload::Event(_)
            | EventPayload::Compaction(_)
            | EventPayload::Overlay(_)
            | EventPayload::Unknown(_) => None,
        });

        for delta in iter {
            config_delta::fold(&mut partial, delta)?;
        }

        Ok(partial)
    }

    /// Dotted config paths explicitly cleared and not subsequently set.
    ///
    /// Reset deltas discard earlier clears.
    /// A clear followed by a replacement in the same apply delta is not
    /// included.
    /// Unknown paths are ignored.
    ///
    /// # Errors
    ///
    /// Returns an error if a delta cannot be folded onto the accumulated state.
    pub fn config_unsets(&self) -> Result<Vec<String>, StreamError> {
        let mut unsets = BTreeSet::new();
        for delta in self.config_deltas() {
            match delta {
                ConfigDelta::Apply(apply) => unsets.extend(apply.unsets.iter().cloned()),
                ConfigDelta::Reset(_) => unsets.clear(),
            }
        }

        let partial = self.config_partial()?;
        Ok(unsets
            .into_iter()
            .filter(|path| {
                // Probe the typed path before resolution injects defaults. Clearing
                // it changes nothing only when the accumulated field is still absent.
                let mut cleared = partial.clone();
                cleared.unset(path).is_ok() && cleared == partial
            })
            .collect())
    }

    /// Removes all events from the end of the stream, until a [`ChatRequest`]
    /// is found, returning that request.
    ///
    /// [`ConfigDelta`] and [`Compaction`] overlays encountered while trimming
    /// are preserved (re-appended) rather than discarded — they apply to the
    /// conversation as a whole, not to the turn being replayed.
    #[must_use]
    pub fn trim_chat_request(&mut self) -> Option<ChatRequest> {
        let mut preserved = Vec::new();
        let mut request = None;

        while let Some(internal) = self.events.pop() {
            match internal.scope() {
                EventScope::Global => preserved.push(internal),
                EventScope::Turn => {
                    if let Some(req) = internal
                        .into_event()
                        .and_then(ConversationEvent::into_chat_request)
                    {
                        request = Some(req);
                        break;
                    }
                    // A non-chat-request conversation event (response,
                    // reasoning, tool call) — part of the replayed turn; drop.
                }
            }
        }

        // Re-append preserved overlays in their original order, whether or not
        // a request was found, so they survive the trim.
        //
        // Appended directly rather than through `adopt`: these are the entries
        // just popped, so `event_ids` still holds their IDs and `adopt` would
        // read that as a collision and reassign them.
        preserved.reverse();
        self.events.append(&mut preserved);

        request
    }

    /// Append a config delta to the stream.
    ///
    /// The delta is recorded as given.
    /// It states which values to merge and which fields to clear first, and
    /// folding it over the accumulated config state is what applies it.
    ///
    /// A delta is not a snapshot: it says how a config changes, not what the
    /// config is.
    /// Diffing one a second time reads its values as a complete state and
    /// changes their meaning — an appended list element becomes the whole
    /// list, and a field left alone becomes a field emptied.
    /// A caller that wants the conversation to reach a particular resolved
    /// config therefore computes that diff once, against the state it means to
    /// diff from, and hands the result here.
    ///
    /// An [`Apply`] that merges nothing and clears nothing is dropped: it
    /// leaves the config as it was, so recording it would put an event in the
    /// conversation that changes nothing.
    /// A [`Reset`] is always appended — it carries no values to be empty of,
    /// and its presence in the stream is the point.
    ///
    /// [`Apply`]: ConfigDelta::Apply
    /// [`Reset`]: ConfigDelta::Reset
    pub fn add_config_delta(&mut self, delta: impl Into<ConfigDelta>) {
        let delta = delta.into();

        if let ConfigDelta::Apply(apply) = &delta
            && apply.delta.is_empty()
            && apply.unsets.is_empty()
        {
            return;
        }

        self.append(EventPayload::ConfigDelta(delta));
    }

    /// Append a config reset point followed by the state layered on top of it.
    ///
    /// A [`Reset`] discards the accumulated config state, so everything the
    /// conversation still needs is restated in the layers above it.
    /// Taking those layers alongside the reset keeps the sequence in one call,
    /// rather than leaving a stream that resolves to program defaults between
    /// two of them.
    ///
    /// Each layer is appended by [`Self::add_config_delta`], which drops the
    /// ones carrying nothing.
    ///
    /// [`Reset`]: ConfigDelta::Reset
    pub fn add_config_reset(
        &mut self,
        reset: ResetDelta,
        layers: impl IntoIterator<Item = ApplyDelta>,
    ) {
        self.add_config_delta(reset);

        for layer in layers {
            self.add_config_delta(layer);
        }
    }

    /// Add a config delta to the stream.
    #[must_use]
    pub fn with_config_delta(mut self, delta: impl Into<ConfigDelta>) -> Self {
        self.add_config_delta(delta);
        self
    }

    /// Add a compaction overlay to the stream.
    pub fn add_compaction(&mut self, compaction: Compaction) {
        self.append(EventPayload::Compaction(compaction));
    }

    /// Append a patch overlay, returning how many events its patches change in
    /// the projected stream.
    ///
    /// The stored events are left untouched; the patches take effect when the
    /// stream is projected for a request.
    ///
    /// A return value of `0` means the projection is unchanged, so a caller
    /// retrying a rejected request would send exactly what it sent before.
    /// That happens when the patches match nothing, or when an earlier overlay
    /// already removed the same metadata.
    pub fn add_overlay(&mut self, patches: Vec<OverlayPatch>) -> usize {
        let changed = self.count_overlay_changes(&patches);

        self.append(EventPayload::Overlay(EventOverlay {
            timestamp: Utc::now(),
            patches,
        }));

        changed
    }

    /// How many events `patches` would change, given the overlays already in
    /// the stream.
    fn count_overlay_changes(&self, patches: &[OverlayPatch]) -> usize {
        let existing: Vec<&OverlayPatch> = self
            .events
            .iter()
            .filter_map(|e| match &e.payload {
                EventPayload::Overlay(overlay) => Some(&overlay.patches),
                _ => None,
            })
            .flatten()
            .collect();

        self.events
            .iter()
            .filter_map(InternalEvent::as_event)
            .filter(|event| {
                // Project this event's metadata through the existing overlays
                // first: metadata an earlier overlay already dropped is not
                // there to drop again.
                let mut metadata = event.metadata.clone();
                for patch in &existing {
                    patch.apply(&mut metadata);
                }

                patches.iter().any(|patch| patch.apply(&mut metadata))
            })
            .count()
    }

    /// Remove all compaction events from the stream.
    ///
    /// Returns the number of compaction events removed.
    pub fn remove_compactions(&mut self) -> usize {
        let before = self.events.len();
        self.events
            .retain(|e| !matches!(&e.payload, EventPayload::Compaction(_)));
        before - self.events.len()
    }

    /// Remove a single compaction event, addressed by its 0-based position
    /// among the compaction events in the stream.
    ///
    /// Returns the removed event, or `None` when the stream holds fewer
    /// compaction events than that (in which case the stream is unchanged).
    pub fn remove_compaction(&mut self, index: usize) -> Option<Compaction> {
        let position = self
            .events
            .iter()
            .enumerate()
            .filter(|(_, event)| matches!(&event.payload, EventPayload::Compaction(_)))
            .map(|(position, _)| position)
            .nth(index)?;

        match self.events.remove(position).payload {
            EventPayload::Compaction(compaction) => Some(compaction),
            _ => unreachable!("position points at a compaction event"),
        }
    }

    /// Returns an iterator over the [`Compaction`] events in the stream.
    pub fn compactions(&self) -> impl Iterator<Item = &Compaction> {
        self.events.iter().filter_map(|e| match &e.payload {
            EventPayload::Compaction(c) => Some(c),
            _ => None,
        })
    }

    /// Returns an iterator over the [`ConfigDelta`] events in the stream.
    pub fn config_deltas(&self) -> impl Iterator<Item = &ConfigDelta> {
        self.events.iter().filter_map(|e| match &e.payload {
            EventPayload::ConfigDelta(delta) => Some(delta),
            _ => None,
        })
    }

    /// Returns an iterator over the [`EventOverlay`] events in the stream.
    pub fn overlays(&self) -> impl Iterator<Item = &EventOverlay> {
        self.events.iter().filter_map(|e| match &e.payload {
            EventPayload::Overlay(o) => Some(o),
            _ => None,
        })
    }

    /// Append every event from `other`, overlays and config deltas included.
    ///
    /// [`Extend`] copies conversation events only, because it consumes an
    /// iterator over them: it suits moving events between streams whose config
    /// state differs, and recomputes config deltas to suit the target.
    /// This is the tool for duplicating a stream wholesale, where compactions,
    /// patch overlays and events this build does not recognize have to survive
    /// as well.
    ///
    /// Appended entries keep their IDs, so a reference into `other` still
    /// resolves against the copy.
    /// An ID this stream has already handed out is replaced on the incoming
    /// entry.
    /// `other`'s config deltas are appended verbatim rather than recomputed, so
    /// the two streams must share a base config for the result to resolve the
    /// same way.
    pub fn append_stream(&mut self, other: Self) {
        for entry in other.events {
            self.adopt(entry);
        }
    }

    /// Apply projection to the stream.
    ///
    /// Reads all patch overlays and compaction overlays and transforms the
    /// event list so that the projected view reflects both: patches rewrite
    /// metadata on the events they match, compactions reduce what a range of
    /// turns contributes.
    /// After this call, the stream's conversation events represent what the LLM
    /// should see.
    ///
    /// Returns one [`TurnOrigin`] per resulting turn, in turn order, mapping
    /// each projected turn back to the raw turn number(s) it represents.
    /// When the stream carries neither patch overlays nor compactions, the
    /// events are left unchanged and every turn maps to its own index.
    ///
    /// Apply this to a copy to preserve the raw stream.
    /// Retained entries keep their IDs, which reference the original raw
    /// entries even when projection changes their content in this view.
    ///
    /// Synthetic summary entries receive ephemeral IDs and have no
    /// corresponding entry in `events.json`.
    /// Do not use those synthetic IDs as references into the raw stream.
    pub fn apply_projection(&mut self) -> Vec<TurnOrigin> {
        projection::apply(&mut self.events, &mut self.event_ids)
    }

    /// List the items `compaction`'s mechanical policies reach, in stream
    /// order.
    ///
    /// A policy narrowed by a size threshold reaches an unpredictable subset of
    /// its range, so this reports which items it actually selects.
    /// A policy without a threshold reaches everything in range, and a summary
    /// replaces its range rather than selecting from it.
    ///
    /// The stream is not modified.
    #[must_use]
    pub fn affected_items(&self, compaction: &Compaction) -> Vec<AffectedItem> {
        projection::affected_items(&self.events, compaction)
    }

    /// Start a new turn with the given chat request.
    ///
    /// Atomically adds a [`TurnStart`] and the [`ChatRequest`] to the stream.
    /// This is the only public way to create turn boundaries.
    ///
    /// Global events (config deltas, compactions) are position-independent and
    /// invisible to turn iteration ([`Self::iter`] skips them), so no attempt
    /// is made to associate trailing globals with the new turn.
    pub fn start_turn(&mut self, request: impl Into<ChatRequest>) {
        self.push(ConversationEvent::now(TurnStart));
        self.push(ConversationEvent::now(request.into()));
    }

    /// Start a new turn, returning `self` for builder chaining.
    ///
    /// See [`start_turn`].
    ///
    /// [`start_turn`]: Self::start_turn
    #[must_use]
    pub fn with_turn(mut self, request: impl Into<ChatRequest>) -> Self {
        self.start_turn(request);
        self
    }

    /// Get a mutable handle to the current (last) turn.
    ///
    /// If the stream has no turns yet, a [`TurnStart`] is injected
    /// automatically.
    /// Returns a [`TurnMut`] that buffers events until [`build()`] is called.
    ///
    /// [`build()`]: TurnMut::build
    pub fn current_turn_mut(&mut self) -> TurnMut<'_> {
        let has_turn = self
            .events
            .iter()
            .any(|e| matches!(&e.payload, EventPayload::Event(event) if event.is_turn_start()));

        if !has_turn {
            self.push(ConversationEvent::now(TurnStart));
        }

        TurnMut::new(self)
    }

    /// Append a payload, returning the ID the stream assigned it.
    fn append(&mut self, payload: EventPayload) -> EventId {
        let event_id = self.event_ids.fresh();
        self.events.push(InternalEvent {
            event_id: event_id.clone(),
            payload,
        });
        event_id
    }

    /// Insert a payload at `index`, returning the ID the stream assigned it.
    fn insert(&mut self, index: usize, payload: EventPayload) -> EventId {
        let event_id = self.event_ids.fresh();
        self.events.insert(index, InternalEvent {
            event_id: event_id.clone(),
            payload,
        });
        event_id
    }

    /// Append an entry from another stream, keeping its ID when this stream has
    /// not handed that ID out.
    ///
    /// IDs are unique within a stream, so an entry arriving from elsewhere can
    /// keep the identity it already has, and references into the source stream
    /// keep resolving against the copy.
    /// Only a collision with an ID this stream has handed out forces a new one.
    fn adopt(&mut self, entry: InternalEvent) -> EventId {
        let event_id = self.event_ids.claim(entry.event_id);
        self.events.push(InternalEvent {
            event_id: event_id.clone(),
            payload: entry.payload,
        });
        event_id
    }

    /// Append a [`ConversationEvent`], returning the ID the stream assigned it.
    ///
    /// The ID identifies this entry for as long as it is in the stream, and is
    /// persisted with it.
    /// Turn boundaries are not created here; use [`Self::start_turn`].
    ///
    /// There is no way back from an [`EventId`] to the entry holding it yet, so
    /// a caller keeps the returned ID for what it writes elsewhere rather than
    /// to look the entry up again.
    ///
    /// [`Self::push`] is the same append for a caller with no use for the ID.
    pub fn push_event(&mut self, event: impl Into<ConversationEvent>) -> EventId {
        self.append(EventPayload::Event(Box::new(event.into())))
    }

    /// Append entries carrying known IDs.
    ///
    /// IDs are normally assigned by the stream, so a test that needs to name
    /// one builds the entries itself.
    /// Going through here registers those IDs, which is what the uniqueness
    /// invariant on `event_ids` needs; pushing onto `events` directly would
    /// leave the stream able to hand out an ID it already holds.
    #[cfg(test)]
    fn extend_entries(&mut self, entries: impl IntoIterator<Item = InternalEvent>) {
        for entry in entries {
            self.adopt(entry);
        }
    }

    /// Whether `event_ids` still holds every ID the stream's entries carry.
    ///
    /// The invariant that field documents, checked directly.
    /// Watching for a reused ID would not: two generated IDs practically never
    /// collide, so the check would hold whether or not the set was in step.
    #[cfg(test)]
    fn id_set_covers_entries(&self) -> bool {
        self.events
            .iter()
            .all(|entry| self.event_ids.contains(&entry.event_id))
    }

    /// Append a [`ConversationEvent`], discarding the ID it was given.
    ///
    /// [`Self::push_event`] for the many callers that only want the event in
    /// the stream, so they do not each write `let _ =`.
    fn push(&mut self, event: impl Into<ConversationEvent>) {
        self.push_event(event);
    }

    /// Returns the structured output schema for the current turn.
    ///
    /// The schema lives on the first [`ChatRequest`] after the last
    /// [`TurnStart`].
    /// It is set once at the start of a turn and must persist across tool-use
    /// round-trips within that turn.
    /// Interrupt replies (`InterruptAction::Reply`) inject additional
    /// `ChatRequest`s with `schema: None`, so we specifically want the *first*
    /// request in the turn, not the last.
    ///
    /// [`TurnStart`]: crate::event::TurnStart
    #[must_use]
    pub fn schema(&self) -> Option<Map<String, Value>> {
        // Find the last TurnStart, then take the first ChatRequest after it.
        let turn_start = self
            .events
            .iter()
            .rposition(|e| matches!(&e.payload, EventPayload::Event(ev) if ev.is_turn_start()));

        let search_from = turn_start.map_or(0, |pos| pos + 1);

        self.events[search_from..]
            .iter()
            .filter_map(InternalEvent::as_event)
            .find_map(|e| e.as_chat_request())
            .and_then(|req| req.schema.clone())
    }

    /// Find a [`ToolCallResponse`] by ID.
    #[must_use]
    pub fn find_tool_call_response(&self, id: &str) -> Option<&ToolCallResponse> {
        self.events
            .iter()
            .filter_map(InternalEvent::as_event)
            .find_map(|event| match &event.kind {
                EventKind::ToolCallResponse(response) if response.id == id => Some(response),
                _ => None,
            })
    }

    /// Returns the last [`ConversationEvent`] in the stream, wrapped in a
    /// [`ConversationEventWithConfigRef`], containing the [`PartialAppConfig`]
    /// at the time the event was added.
    #[must_use]
    pub fn last(&self) -> Option<ConversationEventWithConfigRef<'_>> {
        self.iter().last()
    }

    /// Similar to [`Self::last`], but returns a mutable reference to the last
    /// event.
    #[must_use]
    pub fn last_mut(&mut self) -> Option<ConversationEventWithConfigMut<'_>> {
        self.iter_mut().last()
    }

    /// Returns the first [`ConversationEvent`] in the stream, wrapped in a
    /// [`ConversationEventWithConfigRef`], containing the [`PartialAppConfig`]
    /// at the time the event was added.
    #[must_use]
    pub fn first(&self) -> Option<ConversationEventWithConfigRef<'_>> {
        self.iter().next()
    }

    /// Pops the last [`ConversationEvent`] from the stream, returning it
    /// wrapped in a [`ConversationEventWithConfig`], containing the
    /// [`PartialAppConfig`] at the time the event was added.
    #[must_use]
    pub fn pop(&mut self) -> Option<ConversationEventWithConfig> {
        // The last conversation event, ignoring any trailing overlays
        // (`ConfigDelta`/`Compaction`) that follow it. Those overlays are left
        // in place rather than discarded — a `Compaction` references turn
        // ranges and applies regardless of tail position.
        let pos = self
            .events
            .iter()
            .rposition(|e| e.scope() == EventScope::Turn)?;

        let config = self
            .last()
            .map_or_else(|| self.base_config.to_partial(), |v| v.config);

        let internal = self.events.remove(pos);
        let event_id = internal.event_id.clone();
        internal
            .into_event()
            .map(|event| ConversationEventWithConfig {
                event_id,
                event,
                config,
            })
    }

    /// Returns the last turn-scoped [`ConversationEvent`] in the stream,
    /// skipping trailing `ConfigDelta`/`Compaction` overlays — the event
    /// [`Self::pop`] would remove, without removing it.
    ///
    /// Read-only counterpart of [`Self::pop`] and [`Self::pop_if`].
    /// Callers that intend to replay an event peek here and defer the
    /// destructive pop to a later commit point, so an abandoned operation never
    /// mutates the stream.
    #[must_use]
    pub fn last_turn_event(&self) -> Option<&ConversationEvent> {
        self.events
            .iter()
            .rev()
            .find_map(|event| match event.scope() {
                EventScope::Turn => event.as_event(),
                EventScope::Global => None,
            })
    }

    /// Similar to [`Self::pop`], but only pops if the predicate returns `true`
    /// for the event [`Self::last_turn_event`] returns.
    pub fn pop_if(
        &mut self,
        f: impl Fn(&ConversationEvent) -> bool,
    ) -> Option<ConversationEventWithConfig> {
        if self.last_turn_event().is_some_and(f) {
            self.pop()
        } else {
            None
        }
    }

    /// Pop trailing events while `f` returns `true`, returning the removed
    /// events in pop order (last in the stream first).
    ///
    /// Built on [`pop_if`]: it stops at the first event that fails the
    /// predicate, so only a trailing run is removed, never matching events
    /// deeper in the stream.
    /// Use this instead of [`retain`] when the removal must be confined to the
    /// tail.
    ///
    /// [`pop_if`]: Self::pop_if
    /// [`retain`]: Self::retain
    pub fn pop_while(
        &mut self,
        f: impl Fn(&ConversationEvent) -> bool,
    ) -> Vec<ConversationEventWithConfig> {
        let mut popped = Vec::new();
        while let Some(event) = self.pop_if(&f) {
            popped.push(event);
        }
        popped
    }

    /// Retains only the [`ConversationEvent`]s that pass the predicate.
    ///
    /// [`ConfigDelta`]s are always preserved: they apply to the conversation as
    /// a whole, regardless of position.
    ///
    /// [`Compaction`] overlays are dropped **selectively** when this call
    /// removes turn-scoped events.
    /// An overlay whose turn range lies entirely before the earliest removed
    /// event is kept: those turns lose no content and aren't renumbered
    /// (nothing earlier was removed), so the overlay's positional anchors stay
    /// valid as-is.
    /// From the earliest removed turn onward a turn may be renumbered or have
    /// lost a covered event, and an overlay there can't be rebased or (for
    /// summaries) re-clipped while anchors are positional ([RFD 097]), so it is
    /// dropped.
    /// This is the single enforcement point for that invariant, so
    /// turn-truncation helpers and the `fork` time filter inherit it without
    /// each tracking overlay validity themselves.
    ///
    /// [RFD 097]: https://jp.computer/rfd/097
    pub fn retain(&mut self, mut f: impl FnMut(&ConversationEvent) -> bool) {
        // Fast path: with no overlays present there's nothing to invalidate, so
        // skip the turn-index bookkeeping.
        if !self
            .events
            .iter()
            .any(|e| matches!(&e.payload, EventPayload::Compaction(_)))
        {
            self.events.retain(|event| match event.scope() {
                EventScope::Global => true,
                EventScope::Turn => event.as_event().is_some_and(&mut f),
            });
            return;
        }

        // Turn index (original numbering) of every entry, so a removal can be
        // attributed to the turn it falls in.
        let turn_indices = projection::assign_turn_indices(&self.events);
        let mut first_removed_turn: Option<usize> = None;
        let mut index = 0;
        self.events.retain(|event| {
            let keep = match event.scope() {
                EventScope::Global => true,
                EventScope::Turn => event.as_event().is_some_and(&mut f),
            };
            if !keep {
                let turn = turn_indices[index];
                first_removed_turn = Some(first_removed_turn.map_or(turn, |m| m.min(turn)));
            }
            index += 1;
            keep
        });

        // Drop only overlays the removal could have invalidated: those whose
        // range reaches the earliest removed turn or beyond.
        if let Some(threshold) = first_removed_turn {
            self.events.retain(|event| match &event.payload {
                EventPayload::Compaction(c) => c.to_turn < threshold,
                _ => true,
            });
        }
    }

    /// Clears the stream of any events, leaving the base configuration intact.
    pub fn clear(&mut self) {
        self.events.clear();
    }

    /// Repairs structural invariants that may be violated after arbitrary
    /// filtering.
    ///
    /// Specifically:
    ///
    /// 1. Drops conversation events before the first [`ChatRequest`],
    ///    preserving [`ConfigDelta`]s and [`TurnStart`]s.
    /// 2. Removes orphaned [`ToolCallResponse`]s whose matching
    ///    [`ToolCallRequest`] is missing.
    /// 3. Injects synthetic error [`ToolCallResponse`]s for
    ///    [`ToolCallRequest`]s that lack a matching response.
    /// 4. Removes orphaned [`InquiryResponse`]s whose matching
    ///    [`InquiryRequest`] is missing.
    /// 5. Removes orphaned [`InquiryRequest`]s whose matching
    ///    [`InquiryResponse`] is missing.
    /// 6. Removes a trailing [`TurnStart`] with no following events (artifact
    ///    of an interrupted turn).
    /// 7. Normalizes [`TurnStart`] events: ensures the stream begins with
    ///    exactly one `TurnStart` and re-indexes all turn starts to a
    ///    zero-based sequence.
    ///
    /// [`InquiryRequest`]: crate::event::InquiryRequest
    /// [`InquiryResponse`]: crate::event::InquiryResponse
    /// [`ToolCallRequest`]: crate::event::ToolCallRequest
    /// [`ToolCallResponse`]: crate::event::ToolCallResponse
    /// [`TurnStart`]: crate::event::TurnStart
    pub fn sanitize(&mut self) {
        self.drop_leading_non_user_events();
        self.remove_orphaned_tool_call_responses();
        self.sanitize_orphaned_tool_calls();
        self.remove_orphaned_inquiry_responses();
        self.remove_orphaned_inquiry_requests();
        self.trim_trailing_empty_turn();
        self.normalize_turn_starts();
    }

    /// Drops conversation events before the first [`ChatRequest`] that would be
    /// invalid as leading content (e.g. assistant responses, tool call
    /// results).
    /// [`ConfigDelta`]s and [`TurnStart`]s are preserved — config deltas
    /// maintain configuration state, and turn markers are invisible to
    /// providers but useful for `--last`.
    fn drop_leading_non_user_events(&mut self) {
        let Some(pos) = self.events.iter().position(
            |e| matches!(&e.payload, EventPayload::Event(event) if event.is_chat_request()),
        ) else {
            return;
        };

        let mut idx = 0;
        self.events.retain(|event| {
            let i = idx;
            idx += 1;
            if i >= pos {
                return true;
            }
            match &event.payload {
                EventPayload::ConfigDelta(_)
                | EventPayload::Compaction(_)
                | EventPayload::Overlay(_)
                | EventPayload::Unknown(_) => true,
                EventPayload::Event(e) => e.is_turn_start(),
            }
        });
    }

    /// Removes [`ToolCallResponse`]s whose ID doesn't match any
    /// [`ToolCallRequest`] in the stream.
    ///
    /// [`ToolCallRequest`]: crate::event::ToolCallRequest
    fn remove_orphaned_tool_call_responses(&mut self) {
        let request_ids: Vec<String> = self
            .events
            .iter()
            .filter_map(InternalEvent::as_event)
            .filter_map(|e| e.as_tool_call_request())
            .map(|r| r.id.clone())
            .collect();

        self.events.retain(|event| {
            if let Some(event) = event.as_event()
                && let Some(response) = event.as_tool_call_response()
            {
                return request_ids.contains(&response.id);
            }
            true
        });
    }

    /// Removes [`InquiryResponse`]s that have no matching [`InquiryRequest`]
    /// **within the same turn**.
    ///
    /// The ID set is scoped to the containing turn so `InquiryId` reuse across
    /// turns cannot cross-satisfy a pair.
    /// When a legacy two-segment ID collides within a turn, requests and
    /// responses pair by order (each request satisfies at most one response);
    /// the unpaired excess is removed.
    ///
    /// [`InquiryRequest`]: crate::event::InquiryRequest
    /// [`InquiryResponse`]: crate::event::InquiryResponse
    fn remove_orphaned_inquiry_responses(&mut self) {
        let mut request_counts: HashMap<(usize, InquiryId), usize> = HashMap::new();
        let mut turn = 0;
        for event in self.events.iter().filter_map(InternalEvent::as_event) {
            if event.is_turn_start() {
                turn += 1;
            } else if let Some(request) = event.as_inquiry_request() {
                *request_counts
                    .entry((turn, request.id.clone()))
                    .or_default() += 1;
            }
        }

        let mut turn = 0;
        self.events.retain(|event| {
            let Some(event) = event.as_event() else {
                return true;
            };
            if event.is_turn_start() {
                turn += 1;
                return true;
            }
            let Some(response) = event.as_inquiry_response() else {
                return true;
            };
            match request_counts.get_mut(&(turn, response.id().clone())) {
                Some(remaining) if *remaining > 0 => {
                    *remaining -= 1;
                    true
                }
                _ => false,
            }
        });
    }

    /// Removes [`InquiryRequest`]s that have no matching [`InquiryResponse`]
    /// **within the same turn** (see
    /// [`Self::remove_orphaned_inquiry_responses`] for the turn-scoping and
    /// order-pairing rationale).
    ///
    /// [`InquiryRequest`]: crate::event::InquiryRequest
    /// [`InquiryResponse`]: crate::event::InquiryResponse
    fn remove_orphaned_inquiry_requests(&mut self) {
        let mut response_counts: HashMap<(usize, InquiryId), usize> = HashMap::new();
        let mut turn = 0;
        for event in self.events.iter().filter_map(InternalEvent::as_event) {
            if event.is_turn_start() {
                turn += 1;
            } else if let Some(response) = event.as_inquiry_response() {
                *response_counts
                    .entry((turn, response.id().clone()))
                    .or_default() += 1;
            }
        }

        let mut turn = 0;
        self.events.retain(|event| {
            let Some(event) = event.as_event() else {
                return true;
            };
            if event.is_turn_start() {
                turn += 1;
                return true;
            }
            let Some(request) = event.as_inquiry_request() else {
                return true;
            };
            match response_counts.get_mut(&(turn, request.id.clone())) {
                Some(remaining) if *remaining > 0 => {
                    *remaining -= 1;
                    true
                }
                _ => false,
            }
        });
    }

    /// Ensures the stream has exactly one leading [`TurnStart`] and that all
    /// `TurnStart` indices form a zero-based sequence.
    ///
    /// After filtering, the stream may have multiple stale `TurnStart`s from
    /// earlier turns piled up at the front, or gaps in the index sequence.
    /// This step:
    ///
    /// - Inserts a `TurnStart(0)` if the stream has events but no leading
    ///   `TurnStart`.
    /// - Removes duplicate `TurnStart`s that precede the first `ChatRequest`
    ///   (keeping only the last one).
    /// - Re-indexes all `TurnStart` events to `0, 1, 2, …`.
    fn normalize_turn_starts(&mut self) {
        if self
            .events
            .iter()
            .all(|e| !matches!(&e.payload, EventPayload::Event(event) if !event.is_turn_start()))
        {
            // Stream has no non-TurnStart events, nothing to normalize.
            return;
        }

        // Find the position of the first ChatRequest.
        let first_chat_pos = self.events.iter().position(
            |e| matches!(&e.payload, EventPayload::Event(event) if event.is_chat_request()),
        );

        // Remove all but the last TurnStart before the first ChatRequest. This
        // collapses multiple stale turn markers from filtered turns into a
        // single one.
        if let Some(chat_pos) = first_chat_pos {
            let leading_turn_starts: Vec<usize> = self.events[..chat_pos]
                .iter()
                .enumerate()
                .filter(|(_, e)| matches!(&e.payload, EventPayload::Event(event) if event.is_turn_start()))
                .map(|(i, _)| i)
                .collect();

            if leading_turn_starts.len() > 1 {
                // Keep the last one, remove the rest.
                let to_remove: Vec<usize> =
                    leading_turn_starts[..leading_turn_starts.len() - 1].to_vec();
                let mut idx = 0;
                self.events.retain(|_| {
                    let i = idx;
                    idx += 1;
                    !to_remove.contains(&i)
                });
            }
        }

        // Ensure there's a TurnStart before the first ChatRequest.
        let first_event_is_turn_start = self
            .events
            .iter()
            .any(|e| matches!(&e.payload, EventPayload::Event(event) if event.is_turn_start()))
            && self.events.iter().position(
                |e| matches!(&e.payload, EventPayload::Event(event) if event.is_turn_start()),
            ) < self.events.iter().position(
                |e| matches!(&e.payload, EventPayload::Event(event) if event.is_chat_request()),
            );

        if !first_event_is_turn_start {
            // Find where to insert (right before the first ChatRequest,
            // or at position 0 if there are no ChatRequests).
            let insert_pos = self
                .events
                .iter()
                .position(
                    |e| matches!(&e.payload, EventPayload::Event(event) if event.is_chat_request()),
                )
                .unwrap_or(0);

            let timestamp = self
                .events
                .get(insert_pos)
                .and_then(InternalEvent::as_event)
                .map_or(DateTime::<Utc>::UNIX_EPOCH, |e| e.timestamp);

            self.insert(
                insert_pos,
                EventPayload::Event(Box::new(ConversationEvent::new(TurnStart, timestamp))),
            );
        }
    }

    /// Injects synthetic [`ToolCallResponse`]s for any [`ToolCallRequest`]s
    /// that lack a matching response.
    ///
    /// This can happen when the user interrupts tool execution (e.g. Ctrl+C →
    /// "save & exit") after the request has been streamed but before responses
    /// are recorded.
    /// Providers such as Anthropic reject streams where a `tool_use` block has
    /// no corresponding `tool_result`.
    ///
    /// The synthetic responses carry an error message explaining the
    /// interruption, preserving the context that a tool call was attempted.
    ///
    /// [`ToolCallRequest`]: crate::event::ToolCallRequest
    pub fn sanitize_orphaned_tool_calls(&mut self) {
        // Collect IDs that already have a response.
        let mut response_ids: Vec<String> = Vec::new();
        for event in &self.events {
            if let Some(event) = event.as_event()
                && let EventKind::ToolCallResponse(resp) = &event.kind
            {
                response_ids.push(resp.id.clone());
            }
        }

        // Walk the events to find orphaned request positions.
        // Collect (index, id) pairs for requests that lack a response.
        #[expect(clippy::needless_collect, reason = "borrow checker")]
        let orphans: Vec<(usize, String, DateTime<Utc>)> = self
            .events
            .iter()
            .enumerate()
            .filter_map(|(i, event)| {
                event.as_event().and_then(|event| {
                    event.as_tool_call_request().and_then(|request| {
                        (!response_ids.contains(&request.id))
                            .then(|| (i, request.id.clone(), event.timestamp))
                    })
                })
            })
            .collect();

        // Insert synthetic responses directly after each orphaned request.
        // Iterate in reverse so earlier indices remain valid.
        for (pos, id, timestamp) in orphans.into_iter().rev() {
            self.insert(
                pos + 1,
                EventPayload::Event(Box::new(ConversationEvent::new(
                    ToolCallResponse {
                        id,
                        result: Err("Tool call was interrupted.".to_string()),
                    },
                    timestamp,
                ))),
            );
        }
    }

    /// Returns a turn-level iterator over the stream.
    ///
    /// Each [`Turn`] groups the events between consecutive [`TurnStart`]
    /// markers.
    /// Events before the first `TurnStart` (if any) form an implicit leading
    /// turn.
    ///
    /// [`TurnStart`]: crate::event::TurnStart
    #[must_use]
    pub fn iter_turns(&self) -> IterTurns<'_> {
        IterTurns::new(self.iter())
    }

    /// Iterate over the conversation events, each tagged with its 0-based turn
    /// index and entry ID.
    ///
    /// Turn boundaries match [`Self::iter_turns`]: a [`TurnStart`] opens a new
    /// turn, and events before the first `TurnStart` form an implicit leading
    /// turn.
    ///
    /// Unlike `iter_turns`, no per-event configuration is resolved.
    /// `iter_turns` clones the accumulated [`PartialAppConfig`] for every event
    /// and materializes the whole stream up front; this walks the events once
    /// and allocates nothing.
    /// Prefer it whenever only event content is needed.
    ///
    /// [`TurnStart`]: crate::event::TurnStart
    pub fn iter_events_by_turn(&self) -> impl Iterator<Item = EventInTurn<'_>> {
        let mut turn = 0;
        let mut seen_event = false;

        self.events.iter().filter_map(move |internal| {
            let EventPayload::Event(event) = &internal.payload else {
                return None;
            };

            // A leading `TurnStart` opens the first turn rather than closing an
            // empty one, matching `IterTurns`.
            if event.is_turn_start() && seen_event {
                turn += 1;
            }
            seen_event = true;

            Some(EventInTurn {
                turn,
                event_id: &internal.event_id,
                event,
            })
        })
    }

    /// Returns the number of turns in the stream.
    ///
    /// A turn is delimited by [`TurnStart`] events.
    /// A stream with no events has 0 turns.
    /// A stream with events but no `TurnStart` has 1 implicit turn.
    ///
    /// [`TurnStart`]: crate::event::TurnStart
    #[must_use]
    pub fn turn_count(&self) -> usize {
        self.iter_events_by_turn()
            .last()
            .map_or(0, |event| event.turn + 1)
    }

    /// Returns the turn that was active at the given time.
    ///
    /// Finds the last turn whose starting timestamp is ≤ `dt`.
    /// Returns `None` if the stream has no turns, or if `dt` is before the
    /// first turn.
    ///
    /// Use [`Turn::index()`] on the result to get the 0-based turn index.
    #[must_use]
    pub fn turn_at_time(&self, dt: DateTime<Utc>) -> Option<Turn<'_>> {
        let mut result = None;
        for turn in self.iter_turns() {
            let start = turn.iter().next()?.event.timestamp;
            if start <= dt {
                result = Some(turn);
            } else {
                break;
            }
        }
        result
    }

    /// Retain only the last `n` turns, dropping earlier ones.
    ///
    /// A turn is delimited by a [`TurnStart`] event.
    /// If there are `n` or fewer turns, the stream is left unchanged.
    ///
    /// Dropping leading turns renumbers the remaining ones; [`Self::retain`]
    /// drops any [`Compaction`] overlays when it removes turns (see its docs
    /// for why overlays can't be rebased while anchors are positional).
    ///
    /// [`TurnStart`]: crate::event::TurnStart
    pub fn retain_last_turns(&mut self, n: usize) {
        if n == 0 {
            self.retain(|_| false);
            return;
        }

        let turn_count = self
            .events
            .iter()
            .filter(|e| matches!(&e.payload, EventPayload::Event(ev) if ev.is_turn_start()))
            .count();

        if turn_count <= n {
            return;
        }

        let skip = turn_count - n;
        let mut turns_seen = 0;
        let mut keeping = false;

        self.retain(|event| {
            if event.is_turn_start() {
                turns_seen += 1;
                if turns_seen > skip {
                    keeping = true;
                }
            }
            keeping
        });
    }

    /// Retain only the turns whose 0-based index satisfies `keep`, dropping the
    /// rest.
    ///
    /// Indices are the stream's numbering *before* any removal, so a predicate
    /// built from resolved turn positions selects the turns the caller meant.
    /// A predicate that keeps every turn leaves the stream untouched.
    ///
    /// Dropping turns renumbers the survivors; [`Self::retain`] drops any
    /// [`Compaction`] overlay reaching into a removed turn, while an overlay
    /// confined to an untouched leading block survives.
    pub fn retain_turns(&mut self, keep: impl Fn(usize) -> bool) {
        if (0..self.turn_count()).all(&keep) {
            return;
        }

        let mut turn: usize = 0;
        // Whether the current turn already holds a conversation event. A
        // `TurnStart` opens a new turn only when this is set, which is the
        // boundary rule [`Self::iter_turns`] and
        // `projection::assign_turn_indices` use: events before the first
        // `TurnStart` form an implicit turn 0, and the first explicit
        // `TurnStart` opens turn 1. Numbering the turns any other way here
        // would drop the turns the caller's predicate meant to keep.
        let mut current_has_event = false;

        self.retain(|event| {
            if event.is_turn_start() && current_has_event {
                turn += 1;
            }
            current_has_event = true;
            keep(turn)
        });
    }

    /// Removes a trailing [`TurnStart`] event if it is the last conversation
    /// event in the stream.
    ///
    /// This cleans up empty turns that can occur when the turn loop errors out
    /// before any real events are added after the turn marker.
    pub fn trim_trailing_empty_turn(&mut self) {
        // Walk backwards past any config deltas to find the last real event.
        if let Some(pos) = self
            .events
            .iter()
            .rposition(|e| matches!(&e.payload, EventPayload::Event(_)))
            && let EventPayload::Event(event) = &self.events[pos].payload
            && event.is_turn_start()
        {
            self.events.remove(pos);
        }
    }

    /// Returns an iterator over the events in the stream, wrapped in a
    /// [`ConversationEventWithConfigRef`], containing the [`PartialAppConfig`]
    /// at the time the event was added.
    #[must_use]
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = ConversationEventWithConfigRef<'_>> {
        Iter {
            stream: self,
            front_config: self.base_config.to_partial(),
            front: 0,
            back: self.events.len(),
        }
    }

    /// Similar to [`Self::iter`], but returns a mutable iterator over the
    /// events in the stream.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = ConversationEventWithConfigMut<'_>> {
        IterMut {
            iter: self.events.iter_mut(),
            front_config: self.base_config.to_partial(),
        }
    }

    /// Return a default conversation stream for testing purposes.
    ///
    /// This CANNOT be used in release mode.
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    #[must_use]
    pub fn new_test() -> Self {
        use chrono::TimeZone as _;

        Self {
            base_config: AppConfig::new_test().into(),
            events: vec![],
            event_ids: EventIds::default(),
            duplicated_event_ids: HashSet::new(),
            created_at: Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
        }
    }
}

/// The storage boundary: turning a stream into stored JSON and back.
///
/// This is where a file's shortcomings are dealt with — a legacy layout, an
/// entry with no ID, two entries sharing one.
/// A stream reaching the rest of JP has none of them, so everything above can
/// assume its invariants instead of checking them.
impl ConversationStream {
    /// Construct a stream from a base config and serialized events.
    ///
    /// Duplicate entry IDs are repaired on later occurrences, with a warning
    /// for each replacement.
    /// Payloads and timestamps are preserved.
    ///
    /// The storage layer reads `base_config.json` as a raw JSON [`Value`] and
    /// `events.json` as raw JSON values.
    /// All deserialization, including schema-aware stripping of unknown fields
    /// from the base config, stays inside `jp_conversation`.
    ///
    /// `fallback` supplies values for fields the stored config cannot provide,
    /// and is read only when the stored config on its own cannot be finalized.
    /// Pass [`PartialAppConfig::empty()`] to hold the stored config to standing
    /// alone.
    ///
    /// The returned stream has `created_at` set to [`Utc::now()`].
    /// The caller should chain [`.with_created_at()`] to set the correct
    /// creation time from the conversation ID.
    ///
    /// # Errors
    ///
    /// Returns an error if event deserialization fails, or if the config cannot
    /// be finalized even after `fallback` is applied.
    ///
    /// [`.with_created_at()`]: Self::with_created_at
    pub fn from_parts(
        base_config: Value,
        events: Vec<Value>,
        fallback: &PartialAppConfig,
    ) -> Result<Self, StreamError> {
        let base_config = crate::compat::deserialize_partial_config(base_config);

        let stored = events
            .into_iter()
            .map(|v| serde_json::from_value::<StoredEvent>(v).map_err(StreamError::Json))
            .collect::<Result<Vec<_>, _>>()?;

        let mut event_ids = EventIds::default();
        // Every ID the file carries is reserved before any entry is settled, so
        // a generated replacement cannot take one belonging to an entry further
        // down the file.
        event_ids.reserve(stored.iter().filter_map(|e| e.event_id.clone()));

        let mut seen = HashSet::with_capacity(stored.len());
        let mut duplicated_event_ids = HashSet::new();
        let events = stored
            .into_iter()
            .map(|StoredEvent { event_id, payload }| {
                let event_id = match event_id {
                    // A legacy entry has no identity yet; give it one.
                    None => event_ids.fresh(),
                    // The first entry to carry an ID keeps it.
                    Some(id) if seen.insert(id.clone()) => id,
                    // A later one cannot, so it is reassigned and the shared
                    // value recorded: repair restores uniqueness, but it cannot
                    // say which entry a reference to that value meant.
                    Some(id) => {
                        let replacement = event_ids.fresh();
                        warn!(
                            event_id = %id,
                            replacement_event_id = %replacement,
                            "Regenerated duplicate conversation event ID.",
                        );
                        duplicated_event_ids.insert(id);
                        replacement
                    }
                };

                InternalEvent { event_id, payload }
            })
            .collect();

        Ok(Self {
            base_config: finalize_recovered_config(base_config, fallback)?,
            events,
            event_ids,
            duplicated_event_ids,
            created_at: Utc::now(),
        })
    }

    /// The IDs this load found on more than one entry.
    ///
    /// Repair kept the first entry carrying such an ID and reassigned the rest,
    /// which restores uniqueness but cannot say which entry a pre-existing
    /// reference to the shared ID meant.
    /// A feature that resolves references treats a reference to one of these as
    /// unresolved, and must do so within this load cycle: once the repaired
    /// stream is saved the file holds unique IDs, and a later load reports
    /// nothing here.
    #[must_use]
    pub const fn duplicated_event_ids(&self) -> &HashSet<EventId> {
        &self.duplicated_event_ids
    }

    /// Decompose the stream into its storable parts.
    ///
    /// Returns the base config and the serialized events array as raw JSON.
    /// The storage layer writes these to `base_config.json` and `events.json`
    /// respectively.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails.
    pub fn to_parts(&self) -> Result<(Value, Vec<Value>), StreamError> {
        let base_config =
            serde_json::to_value(self.base_config.to_partial()).map_err(StreamError::Json)?;

        let events = self
            .events
            .iter()
            .map(|e| serde_json::to_value(e).map_err(StreamError::Json))
            .collect::<Result<_, _>>()?;

        Ok((base_config, events))
    }

    /// Construct a stream from the legacy on-disk format where the base config
    /// was packed as the first element in the events array.
    ///
    /// Used by the storage layer's backward-compatibility migration path.
    /// If the first element is not a `ConfigDelta`, returns `None`.
    ///
    /// The returned stream has `created_at` set to [`Utc::now()`].
    /// The caller should chain [`.with_created_at()`] to set the correct
    /// creation time from the conversation ID.
    ///
    /// # Errors
    ///
    /// Returns an error if event deserialization or config conversion fails.
    ///
    /// [`.with_created_at()`]: Self::with_created_at
    pub fn from_legacy_events(
        events: Vec<Value>,
        fallback: &PartialAppConfig,
    ) -> Result<Option<Self>, StreamError> {
        if events.is_empty() {
            return Ok(None);
        }

        // Peek at the first element's type tag.
        let tag = events[0]
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        if tag != "config_delta" {
            return Ok(None);
        }

        // Extract the config subtree as the base config value.
        let base_config = config_delta::subtree(&events[0]);

        // Remaining elements are events. from_parts handles compat stripping.
        let events = events.into_iter().skip(1).collect();

        Ok(Some(Self::from_parts(base_config, events, fallback)?))
    }
}

/// Finalize a recovered stored config, repairing it from `fallback` only if
/// what was stored cannot stand on its own.
///
/// A conversation's config is complete by construction: it is written from a
/// resolved [`AppConfig`], and it overrides the workspace rather than
/// inheriting from it.
/// Recovering an unreadable stored config can break that, because a field
/// required to finalize may be among the ones dropped, and the value replacing
/// it has to come from somewhere.
/// `fallback` is that somewhere, read only on the path where the alternative is
/// a conversation that will not load at all.
fn finalize_recovered_config(
    partial: PartialAppConfig,
    fallback: &PartialAppConfig,
) -> Result<Arc<AppConfig>, StreamError> {
    if fallback.is_empty() {
        return Ok(jp_config::util::build(partial)?.into());
    }

    let error = match jp_config::util::build(partial.clone()) {
        Ok(config) => return Ok(config.into()),
        Err(error) => error,
    };

    warn!(
        %error,
        "Stored conversation config cannot be finalized; filling the gap from the workspace \
         config. The filled value is written back the next time the conversation is saved.",
    );

    Ok(jp_config::util::build(partial.fill_from(fallback.clone()))?.into())
}

/// Error type for the [`ConversationStream`] type and its methods.
#[derive(Debug, thiserror::Error)]
pub enum StreamError {
    /// A [`ConversationStream`] cannot be initialized from an empty iterator,
    /// as it requires the first event to be a [`ConfigDelta`] containing a
    /// valid configuration.
    #[error("Cannot initialize conversation stream from empty iterator.")]
    FromEmptyIterator,

    /// An error occurred for the stream [`AppConfig`].
    #[error(transparent)]
    Config(#[from] jp_config::ConfigError),

    /// Building the stream's [`AppConfig`] failed.
    ///
    /// Covers everything `jp_config::util::build` does: filling defaults,
    /// converting and validating the partial, resolving model aliases, and
    /// ordering instructions and prompt sections.
    /// Conversion and validation failures arrive here too, wrapped as
    /// `jp_config::Error::Schematic`; an alias that resolves to nothing is the
    /// most common cause.
    ///
    /// [`Config`] covers the earlier step: merging deltas into the partial.
    ///
    /// [`Config`]: Self::Config
    #[error(transparent)]
    BuildConfig(#[from] jp_config::Error),

    /// A JSON serialization or deserialization error.
    #[error(transparent)]
    Json(serde_json::Error),

    /// A [`ToolCallResponse`] was pushed without a matching [`ToolCallRequest`]
    /// in the stream.
    ///
    /// [`ToolCallRequest`]: crate::event::ToolCallRequest
    #[error("ToolCallResponse references unknown request ID `{id}`")]
    OrphanedToolCallResponse {
        /// The unmatched response ID.
        id: String,
    },

    /// A [`ToolCallResponse`] was pushed but one with the same ID already
    /// exists in the stream.
    #[error("Duplicate ToolCallResponse for ID `{id}`")]
    DuplicateToolCallResponse {
        /// The duplicated response ID.
        id: String,
    },

    /// An [`InquiryResponse`] was pushed without a matching [`InquiryRequest`]
    /// in the stream.
    ///
    /// [`InquiryRequest`]: crate::event::InquiryRequest
    /// [`InquiryResponse`]: crate::event::InquiryResponse
    #[error("InquiryResponse references unknown request ID `{id}`")]
    OrphanedInquiryResponse {
        /// The unmatched response ID.
        id: String,
    },

    /// An [`InquiryResponse`] was pushed but one with the same ID already
    /// exists in the stream.
    ///
    /// [`InquiryResponse`]: crate::event::InquiryResponse
    #[error("Duplicate InquiryResponse for ID `{id}`")]
    DuplicateInquiryResponse {
        /// The duplicated response ID.
        id: String,
    },
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "stream/event_id_tests.rs"]
mod event_id_tests;

#[cfg(test)]
#[path = "stream/event_id_repair_tests.rs"]
mod event_id_repair_tests;
