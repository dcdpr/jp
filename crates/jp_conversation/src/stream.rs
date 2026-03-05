//! See [`ConversationStream`].

use std::sync::Arc;

use chrono::{DateTime, Utc};
use jp_config::{AppConfig, Config as _, PartialAppConfig, PartialConfig as _};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use tracing::error;

use crate::{
    event::{
        ChatRequest, ChatResponse, ConversationEvent, EventKind, InquiryId, InquiryRequest,
        InquiryResponse, ToolCallRequest, ToolCallResponse, TurnStart,
    },
    storage::{decode_event_value, encode_event},
};

/// An internal representation of events in a conversation stream.
///
/// This type handles base64-encoding of content fields (tool arguments, tool
/// response content, metadata) during serialization, and decoding during
/// deserialization. This keeps the encoding concern isolated to the storage
/// layer — the inner [`ConversationEvent`] types serialize as plain text.
#[derive(Debug, Clone, PartialEq)]
pub enum InternalEvent {
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
    /// Create a new [`InternalEvent::ConfigDelta`].
    pub fn config_delta(delta: impl Into<ConfigDelta>) -> Self {
        Self::ConfigDelta(delta.into())
    }

    /// Convert an internal event into an [`ConversationEvent`]. Returns `None`
    /// if the event is a config delta.
    #[must_use]
    pub fn into_event(self) -> Option<ConversationEvent> {
        match self {
            Self::Event(event) => Some(*event),
            Self::ConfigDelta(_) => None,
        }
    }

    /// Get a reference to [`InternalEvent::Event`], if applicable.
    #[must_use]
    pub fn as_event(&self) -> Option<&ConversationEvent> {
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

        AppConfig::from_partial(partial, vec![]).map_err(Into::into)
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

    /// Push an event with a config delta.
    ///
    /// If the event has a config delta that is not empty, it will be added to
    /// the stream *before* the event is pushed.
    pub fn push_with_config_delta(&mut self, event: impl Into<ConversationEventWithConfig>) {
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
    pub fn push(&mut self, event: impl Into<ConversationEvent>) {
        self.events
            .push(InternalEvent::Event(Box::new(event.into())));
    }

    /// Push a [`ConversationEvent`] of type [`EventKind::ChatRequest`] onto the
    /// stream.
    pub fn add_chat_request(&mut self, event: impl Into<ChatRequest>) {
        self.push(ConversationEvent::now(event.into()));
    }

    /// Add a [`ConversationEvent`] of type [`EventKind::ChatRequest`] onto the
    /// stream.
    #[must_use]
    pub fn with_chat_request(mut self, event: impl Into<ChatRequest>) -> Self {
        self.add_chat_request(event);
        self
    }

    /// Push a [`ConversationEvent`] of type [`EventKind::ChatResponse`] onto
    /// the stream.
    pub fn add_chat_response(&mut self, event: impl Into<ChatResponse>) {
        self.push(ConversationEvent::now(event.into()));
    }

    /// Add a [`ConversationEvent`] of type [`EventKind::ChatResponse`] onto the
    /// stream.
    #[must_use]
    pub fn with_chat_response(mut self, event: impl Into<ChatResponse>) -> Self {
        self.add_chat_response(event);
        self
    }

    /// Push a [`ConversationEvent`] of type [`EventKind::ToolCallRequest`] onto
    /// the stream.
    pub fn add_tool_call_request(&mut self, event: impl Into<ToolCallRequest>) {
        self.push(ConversationEvent::now(event.into()));
    }

    /// Add a [`ConversationEvent`] of type [`EventKind::ToolCallRequest`] onto
    /// the stream.
    #[must_use]
    pub fn with_tool_call_request(mut self, event: impl Into<ToolCallRequest>) -> Self {
        self.add_tool_call_request(event);
        self
    }

    /// Push a [`ConversationEvent`] of type [`EventKind::ToolCallResponse`] onto
    /// the stream.
    pub fn add_tool_call_response(&mut self, event: impl Into<ToolCallResponse>) {
        self.push(ConversationEvent::now(event.into()));
    }

    /// Add a [`ConversationEvent`] of type [`EventKind::ToolCallResponse`] onto
    /// the stream.
    #[must_use]
    pub fn with_tool_call_response(mut self, event: impl Into<ToolCallResponse>) -> Self {
        self.add_tool_call_response(event);
        self
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

    /// Push a [`ConversationEvent`] of type [`EventKind::InquiryRequest`] onto
    /// the stream.
    pub fn add_inquiry_request(&mut self, event: impl Into<InquiryRequest>) {
        self.push(ConversationEvent::now(event.into()));
    }

    /// Add a [`ConversationEvent`] of type [`EventKind::InquiryRequest`] onto
    /// the stream.
    #[must_use]
    pub fn with_inquiry_request(mut self, event: impl Into<InquiryRequest>) -> Self {
        self.add_inquiry_request(event);
        self
    }

    /// Push a [`ConversationEvent`] of type [`EventKind::InquiryResponse`] onto
    /// the stream.
    pub fn add_inquiry_response(&mut self, event: impl Into<InquiryResponse>) {
        self.push(ConversationEvent::now(event.into()));
    }

    /// Add a [`ConversationEvent`] of type [`EventKind::InquiryResponse`] onto
    /// the stream.
    #[must_use]
    pub fn with_inquiry_response(mut self, event: impl Into<InquiryResponse>) -> Self {
        self.add_inquiry_response(event);
        self
    }

    /// Push a [`ConversationEvent`] of type [`EventKind::TurnStart`] onto
    /// the stream.
    pub fn add_turn_start(&mut self) {
        self.push(ConversationEvent::now(TurnStart));
    }

    /// Add a [`ConversationEvent`] of type [`EventKind::TurnStart`] onto the
    /// stream.
    #[must_use]
    pub fn with_turn_start(mut self) -> Self {
        self.add_turn_start();
        self
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
    /// filtering (e.g. `--from`/`--until` on fork).
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
    /// 6. Normalizes [`TurnStart`] events: ensures the stream begins
    ///    with exactly one `TurnStart` and re-indexes all turn starts
    ///    to a zero-based sequence.
    pub fn sanitize(&mut self) {
        self.drop_leading_non_user_events();
        self.remove_orphaned_tool_call_responses();
        self.sanitize_orphaned_tool_calls();
        self.remove_orphaned_inquiry_responses();
        self.remove_orphaned_inquiry_requests();
        self.normalize_turn_starts();
    }

    /// Drops conversation events before the first [`ChatRequest`] that
    /// would be invalid as leading content (e.g. assistant responses,
    /// tool call results). [`ConfigDelta`]s and [`TurnStart`]s are
    /// preserved — config deltas maintain configuration state, and turn
    /// markers are invisible to providers but useful for `--last`.
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

    /// Ensures the stream has exactly one leading [`TurnStart`] and that
    /// all `TurnStart` indices form a zero-based sequence.
    ///
    /// After filtering, the stream may have multiple stale `TurnStart`s
    /// from earlier turns piled up at the front, or gaps in the index
    /// sequence. This step:
    /// - Inserts a `TurnStart(0)` if the stream has events but no
    ///   leading `TurnStart`.
    /// - Removes duplicate `TurnStart`s that precede the first
    ///   `ChatRequest` (keeping only the last one).
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

        // Remove all but the last TurnStart before the first ChatRequest.
        // This collapses multiple stale turn markers from filtered turns
        // into a single one.
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
    /// This can happen when the user interrupts tool execution (e.g. Ctrl+C
    /// → "save & exit") after the request has been streamed but before
    /// responses are recorded. Providers such as Anthropic reject streams
    /// where a `tool_use` block has no corresponding `tool_result`.
    ///
    /// The synthetic responses carry an error message explaining the
    /// interruption, preserving the context that a tool call was attempted.
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

    /// Removes a trailing [`TurnStart`] event if it is the last
    /// conversation event in the stream.
    ///
    /// This cleans up empty turns that can occur when the turn loop errors
    /// out before any real events are added after the turn marker.
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
    /// [`ConversationEventWithConfigRef`], containing the
    /// [`PartialAppConfig`] at the time the event was added.
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

        let mut stream = ConversationStream::new(AppConfig::from_partial(config, vec![])?.into());
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

impl Serialize for ConversationStream {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut stream: Vec<InternalEvent> = Vec::with_capacity(self.events.len() + 1);

        // We store the base config as the first (delta) event in the stream.
        stream.push(InternalEvent::ConfigDelta(ConfigDelta {
            delta: Box::new(self.base_config.to_partial()),
            timestamp: self.created_at,
        }));

        // Then we append all other events in the stream.
        stream.extend(self.events.iter().cloned());
        stream.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ConversationStream {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        let mut events: Vec<InternalEvent> = Vec::deserialize(deserializer)?;
        if events.is_empty() {
            return Err(Error::custom("Cannot deserialize empty event stream"));
        }

        match events.remove(0) {
            InternalEvent::ConfigDelta(base_config) => Ok(Self {
                created_at: base_config.timestamp,
                base_config: AppConfig::from_partial(base_config.into(), vec![])
                    .map_err(Error::custom)?
                    .into(),
                events,
            }),
            InternalEvent::Event(_) => Err(Error::custom(
                "Event stream file is invalid: first event must be a ConfigDelta",
            )),
        }
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
}

// A custom deserializer for `InternalEvent` that avoids serde allocations
// when trying to match `untagged` enum variants.
//
// Deserializes to a JSON `Value` first, then dispatches on the `type` tag.
// This avoids the allocation overhead serde incurs when trying each variant
// of an untagged enum. Base64-encoded storage fields are decoded before the
// final deserialization into typed events.
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
            return serde_json::from_value(value)
                .map(Self::ConfigDelta)
                .map_err(serde::de::Error::custom);
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
