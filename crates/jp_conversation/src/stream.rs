//! See [`ConversationStream`].

use jp_config::{PartialAppConfig, PartialConfig as _};
use serde::{Deserialize, Serialize};
use tracing::error;

use crate::event::{
    ChatRequest, ChatResponse, ConversationEvent, EventKind, InquiryRequest, InquiryResponse,
    ToolCallRequest, ToolCallResponse,
};

/// An internal representation of events in a conversation stream.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
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
    ConfigDelta(Box<PartialAppConfig>),

    /// An event in the conversation stream.
    #[serde(untagged)]
    Event(ConversationEvent),
}

impl InternalEvent {
    /// Convert an internal event into an [`ConversationEvent`]. Returns `None`
    /// if the event is a config delta.
    #[must_use]
    pub fn into_event(self) -> Option<ConversationEvent> {
        match self {
            Self::Event(event) => Some(event),
            Self::ConfigDelta(_) => None,
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
    base_config: PartialAppConfig,

    /// The events in the stream.
    events: Vec<InternalEvent>,
}

impl Default for ConversationStream {
    fn default() -> Self {
        Self::new(PartialAppConfig::empty())
    }
}

impl ConversationStream {
    /// Create a new [`ConversationStream`] with the given base configuration.
    #[must_use]
    pub const fn new(base_config: PartialAppConfig) -> Self {
        Self {
            base_config,
            events: Vec::new(),
        }
    }

    /// Set the base configuration for the stream.
    #[must_use]
    pub fn with_base_config(mut self, base_config: PartialAppConfig) -> Self {
        self.base_config = base_config;
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

    /// Get the merged configuration of the stream.
    ///
    /// This takes the base configuration, and merges all [`ConfigDelta`]
    /// events in the stream from first to last, including any delta's that come
    /// *after* the last conversation event.
    ///
    /// If you need the configuration state of the last event in the stream, use
    /// [`ConversationStream::last`], which returns a
    /// [`ConversationEventWithConfig`]. containing the `config` field for that
    /// event.
    #[must_use]
    pub fn config(&self) -> PartialAppConfig {
        let mut config = self.base_config.clone();
        let iter = self.events.iter().filter_map(|event| match event {
            InternalEvent::ConfigDelta(delta) => Some(*delta.clone()),
            InternalEvent::Event(_) => None,
        });

        for delta in iter {
            if let Err(error) = config.merge(&(), delta) {
                error!(%error, "Failed to merge config delta.");
            }
        }

        config
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
    pub fn add_config_delta(&mut self, delta: impl Into<PartialAppConfig>) {
        self.events
            .push(InternalEvent::ConfigDelta(Box::new(delta.into())));
    }

    /// Add a config delta to the stream.
    #[must_use]
    pub fn with_config_delta(mut self, delta: impl Into<PartialAppConfig>) -> Self {
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
            .map_or_else(|| self.base_config.clone(), |v| v.config);
        let config_delta = last_config.delta(config);

        if !config_delta.is_empty() {
            self.add_config_delta(config_delta);
        }

        self.push(event);
    }

    /// Push a [`ConversationEvent`] onto the stream.
    pub fn push(&mut self, event: impl Into<ConversationEvent>) {
        self.events.push(InternalEvent::Event(event.into()));
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
                    .map_or_else(|| self.base_config.clone(), |v| v.config),

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

    /// Returns an iterator over the events in the stream, wrapped in a
    /// [`ConversationEventWithConfigRef`], containing the
    /// [`PartialAppConfig`] at the time the event was added.
    #[must_use]
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = ConversationEventWithConfigRef<'_>> {
        Iter {
            stream: self,
            front_config: self.base_config.clone(),
            front: 0,
            back: self.events.len(),
        }
    }

    /// Similar to [`Self::iter`], but returns a mutable iterator over the
    /// events in the stream.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = ConversationEventWithConfigMut<'_>> {
        IterMut {
            iter: self.events.iter_mut(),
            front_config: self.base_config.clone(),
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

impl IntoIterator for ConversationStream {
    type Item = ConversationEventWithConfig;

    type IntoIter = IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            current_config: self.base_config.clone(),
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
                    if let Err(error) = self.current_config.merge(&(), *delta) {
                        error!(%error, "Failed to merge config delta.");
                    }
                }
                InternalEvent::Event(event) => {
                    return Some(ConversationEventWithConfig {
                        event,
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
                            && let Err(error) = config.merge(&(), *delta.clone())
                        {
                            error!(%error, "Failed to merge config delta.");
                        }
                    }

                    return Some(ConversationEventWithConfig { event, config });
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
                    if let Err(error) = self.front_config.merge(&(), *delta.clone()) {
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

            let mut config = self.stream.base_config.clone();
            for internal_event in &self.stream.events[..self.back] {
                if let InternalEvent::ConfigDelta(delta) = internal_event
                    && let Err(error) = config.merge(&(), *delta.clone())
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
                    if let Err(error) = self.front_config.merge(&(), *delta.clone()) {
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

    /// The configuration.
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

impl FromIterator<ConversationEventWithConfig> for ConversationStream {
    fn from_iter<T: IntoIterator<Item = ConversationEventWithConfig>>(iter: T) -> Self {
        let mut events = iter.into_iter();
        let (base_config, first_event) = events.next().map_or_else(
            || (PartialAppConfig::empty(), None),
            |e| (e.config, Some(e.event)),
        );

        let mut stream = Self::new(base_config);
        if let Some(event) = first_event {
            stream.push(event);
        }

        stream.extend(events);
        stream
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

impl Serialize for ConversationStream {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut stream: Vec<InternalEvent> = Vec::with_capacity(self.events.len() + 1);

        stream.push(InternalEvent::ConfigDelta(Box::new(
            self.base_config.clone(),
        )));
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
                base_config: *base_config,
                events,
            }),
            InternalEvent::Event(_) => Err(Error::custom(
                "Event stream file is invalid: first event must be a ConfigDelta",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    #[test]
    fn test_conversation_stream_serialization_roundtrip() {
        let mut base_config = PartialAppConfig::empty();
        base_config.conversation.title.generate.auto = Some(false);

        let mut stream = ConversationStream {
            base_config,
            events: vec![],
        };

        insta::assert_json_snapshot!(&stream);

        stream
            .events
            .push(InternalEvent::Event(ConversationEvent::new(
                ChatRequest::from("foo"),
                datetime!(2020-01-01 0:00 utc),
            )));

        stream
            .events
            .push(InternalEvent::Event(ConversationEvent::new(
                ChatResponse::message("bar"),
                datetime!(2020-01-02 0:00 utc),
            )));

        insta::assert_json_snapshot!(&stream);
        let json = serde_json::to_string(&stream).unwrap();
        let stream2 = serde_json::from_str::<ConversationStream>(&json).unwrap();
        assert_eq!(stream, stream2);
    }
}
