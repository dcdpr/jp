//! See [`ConversationStream`].

use std::sync::Arc;

use chrono::{DateTime, Utc};
use jp_config::{AppConfig, PartialAppConfig, PartialConfig as _};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};
use tracing::error;

pub mod turn_iter;
pub mod turn_mut;
pub use turn_iter::{IterTurns, Turn};
pub use turn_mut::TurnMut;

use crate::{
    compat::deserialize_partial_config,
    event::{ChatRequest, ConversationEvent, EventKind, InquiryId, ToolCallResponse, TurnStart},
    storage::{decode_event_value, encode_event},
};

/// An internal representation of events in a conversation stream.
///
/// This type handles base64-encoding of content fields (tool arguments, tool
/// response content, metadata) during serialization, and decoding during
/// deserialization. This keeps the encoding concern isolated to the storage
/// layer — the inner [`ConversationEvent`] types serialize as plain text.
#[derive(Debug, Clone, PartialEq)]
enum InternalEvent {
    /// The configuration state of the conversation is updated.
    ///
    /// When this event is emitted, all subsequent events in the stream are
    /// bound to the new configuration.
    ///
    /// This is a *delta* event, meaning that it is merged on top of all
    /// other `ConfigDelta` events in the stream.
    ///
    /// Any non-config events before the first `ConfigDelta` event are
    /// considered to have the default configuration.
    ConfigDelta(ConfigDelta),
    /// An event in the conversation stream.
    Event(Box<ConversationEvent>),
}

impl Serialize for InternalEvent {
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
        }
    }
}

impl InternalEvent {
    /// Convert an internal event into an [`ConversationEvent`]. Returns `None`
    /// if the event is a config delta.
    #[must_use]
    fn into_event(self) -> Option<ConversationEvent> {
        match self {
            Self::Event(event) => Some(*event),
            Self::ConfigDelta(_) => None,
        }
    }

    /// Get a reference to [`InternalEvent::Event`], if applicable.
    #[must_use]
    fn as_event(&self) -> Option<&ConversationEvent> {
        match self {
            Self::Event(event) => Some(event),
            Self::ConfigDelta(_) => None,
        }
    }
}

/// A configuration delta.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfigDelta {
    /// The timestamp of the event.
    #[serde(
        serialize_with = "crate::serialize_dt",
        deserialize_with = "crate::deserialize_dt"
    )]
    pub timestamp: DateTime<Utc>,

    /// The configuration delta.
    pub delta: Box<PartialAppConfig>,
}

impl ConfigDelta {
    /// Get the [`PartialAppConfig`] delta.
    #[must_use]
    pub fn into_inner(self) -> Box<PartialAppConfig> {
        self.delta
    }
}

impl std::ops::Deref for ConfigDelta {
    type Target = PartialAppConfig;

    fn deref(&self) -> &Self::Target {
        &self.delta
    }
}

impl std::ops::DerefMut for ConfigDelta {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.delta
    }
}

impl AsRef<PartialAppConfig> for ConfigDelta {
    fn as_ref(&self) -> &PartialAppConfig {
        &self.delta
    }
}

impl From<ConfigDelta> for PartialAppConfig {
    fn from(delta: ConfigDelta) -> Self {
        *delta.delta
    }
}

impl From<PartialAppConfig> for ConfigDelta {
    fn from(config: PartialAppConfig) -> Self {
        Self {
            timestamp: Utc::now(),
            delta: Box::new(config),
        }
    }
}

/// Deserialize a [`ConfigDelta`] from a raw JSON value, tolerating schema
/// changes.
///
/// Delegates to [`deserialize_partial_config`] for the `delta` subtree and
/// extracts the timestamp separately.
pub(crate) fn deserialize_config_delta(value: &Value) -> ConfigDelta {
    let delta = value
        .get("delta")
        .cloned()
        .map_or_else(PartialAppConfig::empty, deserialize_partial_config);

    let timestamp = value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|s| crate::parse_dt(s).ok())
        .unwrap_or_else(Utc::now);

    ConfigDelta {
        timestamp,
        delta: Box::new(delta),
    }
}

/// A stream of events that make up a conversation.
#[derive(Debug, PartialEq, Clone)]
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

    /// The timestamp of the creation of the stream.
    pub created_at: DateTime<Utc>,
}

impl ConversationStream {
    /// Create a new [`ConversationStream`] with the given base configuration.
    #[must_use]
    pub fn new(base_config: Arc<AppConfig>) -> Self {
        Self {
            base_config,
            events: Vec::new(),
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

    /// Returns `true` if the stream is empty. This only considers
    /// [`ConversationEvent`]s.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self
            .events
            .iter()
            .any(|e| matches!(e, InternalEvent::Event(_)))
    }

    /// Returns `true` if the stream contains at least one [`ChatRequest`].
    #[must_use]
    pub fn has_chat_request(&self) -> bool {
        self.events
            .iter()
            .any(|e| matches!(e, InternalEvent::Event(event) if event.is_chat_request()))
    }

    /// Returns the number of events in the stream. This only considers
    /// [`ConversationEvent`]s.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events
            .iter()
            .filter(|e| matches!(e, InternalEvent::Event(_)))
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
    /// If you need the configuration state of the last event in the stream, use
    /// [`ConversationStream::last`], which returns a
    /// [`ConversationEventWithConfig`]. containing the `config` field for that
    /// event.
    ///
    /// # Errors
    ///
    /// Returns an error if the merged configuration is invalid.
    pub fn config(&self) -> Result<AppConfig, StreamError> {
        let mut partial = self.base_config.to_partial();
        let iter = self.events.iter().filter_map(|event| match event {
            InternalEvent::ConfigDelta(delta) => Some(delta.clone()),
            InternalEvent::Event(_) => None,
        });

        for delta in iter {
            partial.merge(&(), delta.into())?;
        }

        AppConfig::from_partial_with_defaults(partial).map_err(Into::into)
    }

    /// Removes all events from the end of the stream, until a [`ChatRequest`]
    /// is found, returning that request.
    #[must_use]
    pub fn trim_chat_request(&mut self) -> Option<ChatRequest> {
        loop {
            if let Some(event) = self
                .events
                .pop()?
                .into_event()
                .and_then(ConversationEvent::into_chat_request)
            {
                return Some(event);
            }
        }
    }

    /// Add a config delta to the stream.
    ///
    /// This is a no-op if the delta is empty.
    pub fn add_config_delta(&mut self, delta: impl Into<ConfigDelta>) {
        let ConfigDelta { delta, timestamp } = delta.into();
        let delta = match self.config() {
            Ok(config) => config.to_partial().delta(*delta),
            Err(error) => {
                error!(%error, "Unable to get valid config from conversation stream.");
                return;
            }
        };

        if delta.is_empty() {
            return;
        }

        self.events.push(InternalEvent::ConfigDelta(ConfigDelta {
            delta: Box::new(delta),
            timestamp,
        }));
    }

    /// Add a config delta to the stream.
    #[must_use]
    pub fn with_config_delta(mut self, delta: impl Into<ConfigDelta>) -> Self {
        self.add_config_delta(delta);
        self
    }

    /// Start a new turn with the given chat request.
    ///
    /// Atomically adds a [`TurnStart`] and the [`ChatRequest`] to the stream.
    /// This is the only public way to create turn boundaries.
    pub fn start_turn(&mut self, request: impl Into<ChatRequest>) {
        self.push(ConversationEvent::now(TurnStart));
        self.push(ConversationEvent::now(request.into()));
    }

    /// Start a new turn, returning `self` for builder chaining.
    ///
    /// See [`start_turn`](Self::start_turn).
    #[must_use]
    pub fn with_turn(mut self, request: impl Into<ChatRequest>) -> Self {
        self.start_turn(request);
        self
    }

    /// Get a mutable handle to the current (last) turn.
    ///
    /// If the stream has no turns yet, a [`TurnStart`] is injected
    /// automatically. Returns a [`TurnMut`] that buffers events until
    /// [`build()`](TurnMut::build) is called.
    pub fn current_turn_mut(&mut self) -> TurnMut<'_> {
        let has_turn = self
            .events
            .iter()
            .any(|e| matches!(e, InternalEvent::Event(event) if event.is_turn_start()));

        if !has_turn {
            self.push(ConversationEvent::now(TurnStart));
        }

        TurnMut::new(self)
    }

    /// Push an event with a config delta.
    ///
    /// If the event has a config delta that is not empty, it will be added to
    /// the stream *before* the event is pushed.
    fn push_with_config_delta(&mut self, event: impl Into<ConversationEventWithConfig>) {
        let ConversationEventWithConfig { event, config } = event.into();

        let last_config = self
            .last()
            .map_or_else(|| self.base_config.to_partial(), |v| v.config);
        let config_delta = last_config.delta(config);

        if !config_delta.is_empty() {
            self.add_config_delta(ConfigDelta {
                delta: Box::new(config_delta),
                timestamp: event.timestamp,
            });
        }

        self.push(event);
    }

    /// Push a [`ConversationEvent`] onto the stream.
    fn push(&mut self, event: impl Into<ConversationEvent>) {
        self.events
            .push(InternalEvent::Event(Box::new(event.into())));
    }

    /// Returns the structured output schema for the current turn.
    ///
    /// The schema lives on the first [`ChatRequest`] after the last
    /// [`TurnStart`]. It is set once at the start of a turn and must
    /// persist across tool-use round-trips within that turn. Interrupt
    /// replies (`InterruptAction::Reply`) inject additional
    /// `ChatRequest`s with `schema: None`, so we specifically want the
    /// *first* request in the turn, not the last.
    ///
    /// [`TurnStart`]: crate::event::TurnStart
    #[must_use]
    pub fn schema(&self) -> Option<Map<String, Value>> {
        // Find the last TurnStart, then take the first ChatRequest after it.
        let turn_start = self
            .events
            .iter()
            .rposition(|e| matches!(e, InternalEvent::Event(ev) if ev.is_turn_start()));

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

    /// Pops the last [`ConversationEvent`] from the stream, returning it wrapped
    /// in a [`ConversationEventWithConfig`], containing the
    /// [`PartialAppConfig`] at the time the event was added.
    #[must_use]
    pub fn pop(&mut self) -> Option<ConversationEventWithConfig> {
        loop {
            let config = match self.events.last() {
                // No events, so we're done.
                None => return None,

                // If the last event is a `ConversationEvent`, we handle it.
                Some(InternalEvent::Event(_)) => self
                    .last()
                    .map_or_else(|| self.base_config.to_partial(), |v| v.config),

                // Any other event we remove, and continue.
                _ => {
                    self.events.pop();
                    continue;
                }
            };

            return self.events.pop().and_then(|e| {
                e.into_event()
                    .map(|event| ConversationEventWithConfig { event, config })
            });
        }
    }

    /// Similar to [`Self::pop`], but only pops if the predicate returns `true`.
    pub fn pop_if(
        &mut self,
        f: impl Fn(&ConversationEvent) -> bool,
    ) -> Option<ConversationEventWithConfig> {
        if !self
            .events
            .iter()
            .rev()
            .find_map(|event| match event {
                InternalEvent::Event(event) => Some(f(event)),
                InternalEvent::ConfigDelta(_) => None,
            })
            .unwrap_or(false)
        {
            return None;
        }

        self.pop()
    }

    /// Retains only the [`ConversationEvent`]s that pass the predicate.
    ///
    /// This does NOT remove the [`ConfigDelta`]s.
    pub fn retain(&mut self, mut f: impl FnMut(&ConversationEvent) -> bool) {
        self.events.retain(|event| match event {
            InternalEvent::ConfigDelta(_) => true,
            InternalEvent::Event(event) => f(event),
        });
    }

    /// Clears the stream of any events, leaving the base configuration intact.
    pub fn clear(&mut self) {
        self.events.clear();
    }

    /// Repairs structural invariants that may be violated after arbitrary
    /// filtering.
    ///
    /// Specifically:
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
    /// 6. Removes a trailing [`TurnStart`] with no following events
    ///    (artifact of an interrupted turn).
    /// 7. Normalizes [`TurnStart`] events: ensures the stream begins with
    ///    exactly one `TurnStart` and re-indexes all turn starts to a
    ///    zero-based sequence.
    ///
    /// [`ToolCallRequest`]: crate::event::ToolCallRequest
    /// [`ToolCallResponse`]: crate::event::ToolCallResponse
    /// [`InquiryRequest`]: crate::event::InquiryRequest
    /// [`InquiryResponse`]: crate::event::InquiryResponse
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
    /// results). [`ConfigDelta`]s and [`TurnStart`]s are preserved — config
    /// deltas maintain configuration state, and turn markers are invisible to
    /// providers but useful for `--last`.
    fn drop_leading_non_user_events(&mut self) {
        let Some(pos) = self
            .events
            .iter()
            .position(|e| matches!(e, InternalEvent::Event(event) if event.is_chat_request()))
        else {
            return;
        };

        let mut idx = 0;
        self.events.retain(|event| {
            let i = idx;
            idx += 1;
            if i >= pos {
                return true;
            }
            match event {
                InternalEvent::ConfigDelta(_) => true,
                InternalEvent::Event(e) => e.is_turn_start(),
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

    /// Removes [`InquiryResponse`]s whose ID doesn't match any
    /// [`InquiryRequest`] in the stream.
    ///
    /// [`InquiryResponse`]: crate::event::InquiryResponse
    /// [`InquiryRequest`]: crate::event::InquiryRequest
    fn remove_orphaned_inquiry_responses(&mut self) {
        let request_ids: Vec<InquiryId> = self
            .events
            .iter()
            .filter_map(InternalEvent::as_event)
            .filter_map(|e| e.as_inquiry_request())
            .map(|r| r.id.clone())
            .collect();

        self.events.retain(|event| {
            if let Some(event) = event.as_event()
                && let Some(response) = event.as_inquiry_response()
            {
                return request_ids.contains(&response.id);
            }
            true
        });
    }

    /// Removes [`InquiryRequest`]s whose ID doesn't match any
    /// [`InquiryResponse`] in the stream.
    ///
    /// [`InquiryRequest`]: crate::event::InquiryRequest
    /// [`InquiryResponse`]: crate::event::InquiryResponse
    fn remove_orphaned_inquiry_requests(&mut self) {
        let response_ids: Vec<InquiryId> = self
            .events
            .iter()
            .filter_map(InternalEvent::as_event)
            .filter_map(|e| e.as_inquiry_response())
            .map(|r| r.id.clone())
            .collect();

        self.events.retain(|event| {
            if let Some(event) = event.as_event()
                && let Some(request) = event.as_inquiry_request()
            {
                return response_ids.contains(&request.id);
            }
            true
        });
    }

    /// Ensures the stream has exactly one leading [`TurnStart`] and that all
    /// `TurnStart` indices form a zero-based sequence.
    ///
    /// After filtering, the stream may have multiple stale `TurnStart`s from
    /// earlier turns piled up at the front, or gaps in the index sequence. This
    /// step:
    /// - Inserts a `TurnStart(0)` if the stream has events but no leading
    ///   `TurnStart`.
    /// - Removes duplicate `TurnStart`s that precede the first `ChatRequest`
    ///   (keeping only the last one).
    /// - Re-indexes all `TurnStart` events to `0, 1, 2, …`.
    fn normalize_turn_starts(&mut self) {
        if self
            .events
            .iter()
            .all(|e| !matches!(e, InternalEvent::Event(event) if !event.is_turn_start()))
        {
            // Stream has no non-TurnStart events, nothing to normalize.
            return;
        }

        // Find the position of the first ChatRequest.
        let first_chat_pos = self
            .events
            .iter()
            .position(|e| matches!(e, InternalEvent::Event(event) if event.is_chat_request()));

        // Remove all but the last TurnStart before the first ChatRequest. This
        // collapses multiple stale turn markers from filtered turns into a
        // single one.
        if let Some(chat_pos) = first_chat_pos {
            let leading_turn_starts: Vec<usize> = self.events[..chat_pos]
                .iter()
                .enumerate()
                .filter(|(_, e)| matches!(e, InternalEvent::Event(event) if event.is_turn_start()))
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
        let first_event_is_turn_start =
            self.events
                .iter()
                .any(|e| matches!(e, InternalEvent::Event(event) if event.is_turn_start()))
                && self.events.iter().position(
                    |e| matches!(e, InternalEvent::Event(event) if event.is_turn_start()),
                ) < self.events.iter().position(
                    |e| matches!(e, InternalEvent::Event(event) if event.is_chat_request()),
                );

        if !first_event_is_turn_start {
            // Find where to insert (right before the first ChatRequest,
            // or at position 0 if there are no ChatRequests).
            let insert_pos = self
                .events
                .iter()
                .position(|e| matches!(e, InternalEvent::Event(event) if event.is_chat_request()))
                .unwrap_or(0);

            let timestamp = self
                .events
                .get(insert_pos)
                .and_then(InternalEvent::as_event)
                .map_or(DateTime::<Utc>::UNIX_EPOCH, |e| e.timestamp);

            self.events.insert(
                insert_pos,
                InternalEvent::Event(Box::new(ConversationEvent::new(TurnStart, timestamp))),
            );
        }
    }

    /// Injects synthetic [`ToolCallResponse`]s for any [`ToolCallRequest`]s
    /// that lack a matching response.
    ///
    /// This can happen when the user interrupts tool execution (e.g. Ctrl+C →
    /// "save & exit") after the request has been streamed but before responses
    /// are recorded. Providers such as Anthropic reject streams where a
    /// `tool_use` block has no corresponding `tool_result`.
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
            let response = InternalEvent::Event(Box::new(ConversationEvent::new(
                ToolCallResponse {
                    id,
                    result: Err("Tool call was interrupted.".to_string()),
                },
                timestamp,
            )));
            self.events.insert(pos + 1, response);
        }
    }

    /// Returns a turn-level iterator over the stream.
    ///
    /// Each [`Turn`] groups the events between consecutive [`TurnStart`]
    /// markers. Events before the first `TurnStart` (if any) form an implicit
    /// leading turn.
    ///
    /// [`TurnStart`]: crate::event::TurnStart
    #[must_use]
    pub fn iter_turns(&self) -> IterTurns<'_> {
        IterTurns::new(self.iter())
    }

    /// Retain only the last `n` turns, dropping earlier ones.
    ///
    /// A turn is delimited by a [`TurnStart`] event. If there are `n` or
    /// fewer turns, the stream is left unchanged.
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
            .filter(|e| matches!(e, InternalEvent::Event(ev) if ev.is_turn_start()))
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
            .rposition(|e| matches!(e, InternalEvent::Event(_)))
            && let InternalEvent::Event(ref event) = self.events[pos]
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
            created_at: Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
        }
    }
}

impl Extend<ConversationEventWithConfig> for ConversationStream {
    fn extend<T: IntoIterator<Item = ConversationEventWithConfig>>(&mut self, iter: T) {
        for v in iter {
            self.push_with_config_delta(v);
        }
    }
}

impl Extend<ConversationEvent> for ConversationStream {
    fn extend<T: IntoIterator<Item = ConversationEvent>>(&mut self, iter: T) {
        for v in iter {
            self.push(v);
        }
    }
}

impl IntoIterator for ConversationStream {
    type Item = ConversationEventWithConfig;

    type IntoIter = IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            current_config: self.base_config.to_partial(),
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
            let event = self.inner_iter.next()?;

            match event {
                InternalEvent::ConfigDelta(delta) => {
                    if let Err(error) = self.current_config.merge(&(), delta.into()) {
                        error!(%error, "Failed to merge config delta.");
                    }
                }
                InternalEvent::Event(event) => {
                    return Some(ConversationEventWithConfig {
                        event: *event,
                        config: self.current_config.clone(),
                    });
                }
            }
        }
    }
}

impl DoubleEndedIterator for IntoIter {
    fn next_back(&mut self) -> Option<Self::Item> {
        loop {
            let event = self.inner_iter.next_back()?;

            match event {
                InternalEvent::ConfigDelta(_) => {
                    // A delta at the very end of the list affects nothing that
                    // follows it (because nothing follows it), and it doesn't
                    // affect previous items. We simply discard it.
                }
                InternalEvent::Event(event) => {
                    // Start with the state currently at the front of the line
                    let mut config = self.current_config.clone();

                    // Scan the remaining items in the middle (without consuming
                    // them) to apply all pending deltas to our temporary
                    // config.
                    for internal_event in self.inner_iter.as_slice() {
                        if let InternalEvent::ConfigDelta(delta) = internal_event
                            && let Err(error) = config.merge(&(), delta.clone().into())
                        {
                            error!(%error, "Failed to merge config delta.");
                        }
                    }

                    return Some(ConversationEventWithConfig {
                        event: *event,
                        config,
                    });
                }
            }
        }
    }
}

/// An iterator over the borrowed events in a conversation stream.
struct Iter<'a> {
    /// The stream being iterated over.
    stream: &'a ConversationStream,

    /// The configuration state for the first, next event in the iterator.
    front_config: PartialAppConfig,

    /// The index of the `next` event in the iterator.
    front: usize,

    /// The index of the `next_back` event in the iterator.
    back: usize,
}

impl<'a> Iterator for Iter<'a> {
    type Item = ConversationEventWithConfigRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.front < self.back {
            let event = &self.stream.events[self.front];
            self.front += 1;

            match event {
                InternalEvent::ConfigDelta(delta) => {
                    if let Err(error) = self.front_config.merge(&(), delta.clone().into()) {
                        error!(%error, "Failed to merge config delta.");
                    }
                }
                InternalEvent::Event(event) => {
                    return Some(ConversationEventWithConfigRef {
                        event,
                        config: self.front_config.clone(),
                    });
                }
            }
        }

        None
    }
}

impl DoubleEndedIterator for Iter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        while self.back > self.front {
            self.back -= 1;
            let event = &self.stream.events[self.back];

            let InternalEvent::Event(event) = event else {
                continue;
            };

            let mut config = self.stream.base_config.to_partial();
            for internal_event in &self.stream.events[..self.back] {
                if let InternalEvent::ConfigDelta(delta) = internal_event
                    && let Err(error) = config.merge(&(), delta.clone().into())
                {
                    error!(%error, "Failed to merge config delta.");
                }
            }

            return Some(ConversationEventWithConfigRef { event, config });
        }

        None
    }
}

/// An iterator over the mutable events in a conversation stream.
pub struct IterMut<'a> {
    /// The configuration state for the first, next event in the iterator.
    front_config: PartialAppConfig,

    /// The iterator over the events in the stream.
    iter: std::slice::IterMut<'a, InternalEvent>,
}

impl<'a> Iterator for IterMut<'a> {
    type Item = ConversationEventWithConfigMut<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        for event in self.iter.by_ref() {
            match event {
                InternalEvent::ConfigDelta(delta) => {
                    if let Err(error) = self.front_config.merge(&(), delta.clone().into()) {
                        error!(%error, "Failed to merge config delta.");
                    }
                }
                InternalEvent::Event(event) => {
                    return Some(ConversationEventWithConfigMut {
                        event,
                        config: self.front_config.clone(),
                    });
                }
            }
        }

        None
    }
}

/// A reference to a [`ConversationEvent`] with its configuration.
#[derive(Debug, PartialEq, Clone)]
pub struct ConversationEventWithConfigRef<'a> {
    /// The event.
    pub event: &'a ConversationEvent,

    /// The configuration.
    pub config: PartialAppConfig,
}

/// A mutable reference to a [`ConversationEvent`] with its configuration.
#[derive(Debug, PartialEq)]
pub struct ConversationEventWithConfigMut<'a> {
    /// The event.
    pub event: &'a mut ConversationEvent,

    /// The configuration.
    pub config: PartialAppConfig,
}

/// A [`ConversationEvent`] with its configuration.
#[derive(Debug, PartialEq, Clone)]
pub struct ConversationEventWithConfig {
    /// The event.
    pub event: ConversationEvent,

    /// The configuration at the time the event was added.
    ///
    /// It should be noted that this is not necessarily the same as the
    /// current active configuration of the application, even if this is the
    /// latest event in the stream. For one, the event may have been added a
    /// while ago, but more importantly, not all configuration changes are
    /// automatically applied to a [`ConversationStream`]. For example, if a new
    /// tool is added in the configuration, it will not become available in the
    /// conversation stream until explicitly added using the CLI flag `--tool`
    /// or `--cfg`, while *NEW* conversations *WILL* get the new tool by
    /// default.
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
            event: value.event.clone(),
            config: value.config,
        }
    }
}

impl FromIterator<ConversationEventWithConfig> for Result<ConversationStream, StreamError> {
    fn from_iter<T: IntoIterator<Item = ConversationEventWithConfig>>(iter: T) -> Self {
        let mut events = iter.into_iter();

        let Some((config, first_event)) = events.next().map(|e| (e.config, e.event)) else {
            return Err(StreamError::FromEmptyIterator);
        };

        let mut stream =
            ConversationStream::new(AppConfig::from_partial_with_defaults(config)?.into());
        stream.push(first_event);
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

impl From<ConversationEvent> for ConversationEventWithConfig {
    fn from(event: ConversationEvent) -> Self {
        Self {
            event,
            config: PartialAppConfig::empty(),
        }
    }
}

impl ConversationStream {
    /// Construct a stream from a base config and serialized events.
    ///
    /// The storage layer reads `base_config.json` as a raw JSON [`Value`] and
    /// `events.json` as raw JSON values. All deserialization, including
    /// schema-aware stripping of unknown fields from the base config, stays
    /// inside `jp_conversation`.
    ///
    /// The returned stream has `created_at` set to [`Utc::now()`]. The caller
    /// should chain [`.with_created_at()`] to set the correct creation time
    /// from the conversation ID.
    ///
    /// [`.with_created_at()`]: Self::with_created_at
    ///
    /// # Errors
    ///
    /// Returns an error if event deserialization or config conversion fails.
    pub fn from_parts(base_config: Value, events: Vec<Value>) -> Result<Self, StreamError> {
        let base_config = crate::compat::deserialize_partial_config(base_config);

        let events = events
            .into_iter()
            .map(|v| serde_json::from_value(v).map_err(StreamError::Json))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            base_config: AppConfig::from_partial_with_defaults(base_config)?.into(),
            events,
            created_at: Utc::now(),
        })
    }

    /// Decompose the stream into its storable parts.
    ///
    /// Returns the base config and the serialized events array as raw JSON. The
    /// storage layer writes these to `base_config.json` and `events.json`
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
    /// Used by the storage layer's backward-compatibility migration path. If
    /// the first element is not a `ConfigDelta`, returns `None`.
    ///
    /// # Errors
    ///
    /// Returns an error if event deserialization or config conversion fails.
    pub fn from_legacy_events(events: Vec<Value>) -> Result<Option<Self>, StreamError> {
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

        // Extract timestamp from the first element.
        let created_at = events[0]
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|s| crate::parse_dt(s).ok())
            .unwrap_or_else(Utc::now);

        // Extract the delta subtree as the base config value.
        let base_config = events[0]
            .get("delta")
            .cloned()
            .unwrap_or(Value::Object(Map::default()));

        // Remaining elements are events. from_parts handles compat stripping.
        let events = events.into_iter().skip(1).collect();
        let mut stream = Self::from_parts(base_config, events)?;
        stream.created_at = created_at;

        Ok(Some(stream))
    }
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
    /// [`InquiryResponse`]: crate::event::InquiryResponse
    /// [`InquiryRequest`]: crate::event::InquiryRequest
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

// A custom deserializer for `InternalEvent` that avoids serde allocations when
// trying to match `untagged` enum variants.
//
// Deserializes to a JSON `Value` first, then dispatches on the `type` tag. This
// avoids the allocation overhead serde incurs when trying each variant of an
// untagged enum. Base64-encoded storage fields are decoded before the final
// deserialization into typed events.
//
// `cargo dhat` had shown the untagged approach to be a hotspot.
impl<'de> Deserialize<'de> for InternalEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut value = Value::deserialize(deserializer)?;

        let tag = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();

        if tag == "config_delta" {
            return Ok(Self::ConfigDelta(deserialize_config_delta(&value)));
        }

        // Decode base64-encoded storage fields before deserializing.
        decode_event_value(&mut value);

        serde_json::from_value(value)
            .map(|e| Self::Event(Box::new(e)))
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
