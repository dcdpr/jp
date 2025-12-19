//! See [`ConversationStream`].

use jp_config::{AppConfig, Config as _, PartialAppConfig, PartialConfig as _};
use serde::{Deserialize, Serialize};
use time::UtcDateTime;
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
    ConfigDelta(ConfigDelta),
    // ConfigDelta(Box<PartialAppConfig>),
    /// An event in the conversation stream.
    #[serde(untagged)]
    Event(Box<ConversationEvent>),
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
    pub timestamp: UtcDateTime,

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
            timestamp: UtcDateTime::now(),
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
    base_config: AppConfig,

    /// The events in the stream.
    events: Vec<InternalEvent>,

    /// The timestamp of the creation of the stream.
    created_at: UtcDateTime,
}

impl ConversationStream {
    /// Create a new [`ConversationStream`] with the given base configuration.
    #[must_use]
    pub fn new(base_config: AppConfig) -> Self {
        Self {
            base_config,
            events: Vec::new(),
            created_at: UtcDateTime::now(),
        }
    }

    /// Set the base configuration for the stream.
    #[must_use]
    pub fn with_base_config(mut self, base_config: AppConfig) -> Self {
        self.base_config = base_config;
        self
    }

    /// Set the timestamp of the creation of the stream.
    #[must_use]
    pub fn with_created_at(mut self, created_at: impl Into<UtcDateTime>) -> Self {
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

        AppConfig::from_partial(partial).map_err(Into::into)
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

    /// Clears the stream of any events, leaving the base configuration intact.
    pub fn clear(&mut self) {
        self.events.clear();
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

        let mut stream = ConversationStream::new(AppConfig::from_partial(config)?);
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
                base_config: AppConfig::from_partial(base_config.into()).map_err(Error::custom)?,
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

#[cfg(test)]
mod tests {
    use jp_config::{
        conversation::tool::RunMode,
        model::id::{PartialModelIdConfig, ProviderId},
    };
    use time::macros::datetime;

    use super::*;

    #[test]
    fn test_conversation_stream_serialization_roundtrip() {
        let mut base_config = PartialAppConfig::empty();
        base_config.conversation.title.generate.auto = Some(false);
        base_config.conversation.tools.defaults.run = Some(RunMode::Ask);
        base_config.assistant.model.id = PartialModelIdConfig {
            provider: Some(ProviderId::Anthropic),
            name: Some("test".parse().unwrap()),
        }
        .into();

        let mut stream = ConversationStream {
            base_config: AppConfig::from_partial(base_config).unwrap(),
            events: vec![],
            created_at: datetime!(2020-01-01 0:00 utc).into(),
        };

        insta::assert_json_snapshot!(&stream);

        stream
            .events
            .push(InternalEvent::Event(Box::new(ConversationEvent::new(
                ChatRequest::from("foo"),
                datetime!(2020-01-01 0:00 utc),
            ))));

        stream
            .events
            .push(InternalEvent::Event(Box::new(ConversationEvent::new(
                ChatResponse::message("bar"),
                datetime!(2020-01-02 0:00 utc),
            ))));

        insta::assert_json_snapshot!(&stream);
        let json = serde_json::to_string(&stream).unwrap();
        let stream2 = serde_json::from_str::<ConversationStream>(&json).unwrap();
        assert_eq!(stream, stream2);
    }
}
