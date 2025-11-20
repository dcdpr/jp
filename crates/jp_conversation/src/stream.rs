use jp_config::{PartialAppConfig, PartialConfig as _};
use serde::{Deserialize, Serialize};
use tracing::error;

use crate::event::{
    ChatRequest, ChatResponse, ConfigDelta, ConversationEvent, EventKind, InquiryRequest,
    InquiryResponse, ToolCallRequest, ToolCallResponse,
};

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

    #[serde(untagged)]
    Event(ConversationEvent),
}

impl InternalEvent {
    #[must_use]
    pub fn into_event(self) -> Option<ConversationEvent> {
        match self {
            InternalEvent::Event(event) => Some(event),
            InternalEvent::ConfigDelta(_) => None,
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub struct ConversationStream {
    base_config: PartialAppConfig,
    events: Vec<InternalEvent>,
}

impl Default for ConversationStream {
    fn default() -> Self {
        Self::new(PartialAppConfig::empty())
    }
}

impl ConversationStream {
    #[must_use]
    pub fn new(base_config: PartialAppConfig) -> Self {
        Self {
            base_config,
            events: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_base_config(mut self, base_config: PartialAppConfig) -> Self {
        self.base_config = base_config;
        self
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self
            .events
            .iter()
            .any(|e| matches!(e, InternalEvent::Event(_)))
    }

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
            InternalEvent::ConfigDelta(ConfigDelta(delta)) => Some(*delta.clone()),
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

    pub fn add_config_delta(&mut self, delta: impl Into<ConfigDelta>) {
        self.events.push(InternalEvent::ConfigDelta(delta.into()));
    }

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

        let last_config = self.last().map_or(self.base_config.clone(), |v| v.config);
        let config_delta = last_config.delta(config);

        if !config_delta.is_empty() {
            self.add_config_delta(config_delta);
        }

        self.push(event);
    }

    pub fn push(&mut self, event: impl Into<ConversationEvent>) {
        self.events.push(InternalEvent::Event(event.into()));
    }

    pub fn add_chat_request(&mut self, event: impl Into<ChatRequest>) {
        self.push(ConversationEvent::now(event.into()));
    }

    #[must_use]
    pub fn with_chat_request(mut self, event: impl Into<ChatRequest>) -> Self {
        self.add_chat_request(event);
        self
    }

    pub fn add_chat_response(&mut self, event: impl Into<ChatResponse>) {
        self.push(ConversationEvent::now(event.into()));
    }

    #[must_use]
    pub fn with_chat_response(mut self, event: impl Into<ChatResponse>) -> Self {
        self.add_chat_response(event);
        self
    }

    pub fn add_tool_call_request(&mut self, event: impl Into<ToolCallRequest>) {
        self.push(ConversationEvent::now(event.into()));
    }

    #[must_use]
    pub fn with_tool_call_request(mut self, event: impl Into<ToolCallRequest>) -> Self {
        self.add_tool_call_request(event);
        self
    }

    pub fn add_tool_call_response(&mut self, event: impl Into<ToolCallResponse>) {
        self.push(ConversationEvent::now(event.into()));
    }

    #[must_use]
    pub fn with_tool_call_response(mut self, event: impl Into<ToolCallResponse>) -> Self {
        self.add_tool_call_response(event);
        self
    }

    pub fn add_inquiry_request(&mut self, event: impl Into<InquiryRequest>) {
        self.push(ConversationEvent::now(event.into()));
    }

    #[must_use]
    pub fn with_inquiry_request(mut self, event: impl Into<InquiryRequest>) -> Self {
        self.add_inquiry_request(event);
        self
    }

    pub fn add_inquiry_response(&mut self, event: impl Into<InquiryResponse>) {
        self.push(ConversationEvent::now(event.into()));
    }

    #[must_use]
    pub fn with_inquiry_response(mut self, event: impl Into<InquiryResponse>) -> Self {
        self.add_inquiry_response(event);
        self
    }

    #[must_use]
    pub fn last(&self) -> Option<ConversationEventWithConfigRef<'_>> {
        self.iter().last()
    }

    #[must_use]
    pub fn last_mut(&mut self) -> Option<ConversationEventWithConfigMut<'_>> {
        self.iter_mut().last()
    }

    #[must_use]
    pub fn first(&self) -> Option<ConversationEventWithConfigRef<'_>> {
        self.iter().next()
    }

    #[must_use]
    pub fn pop(&mut self) -> Option<ConversationEventWithConfig> {
        loop {
            let config = match self.events.last() {
                // No events, so we're done.
                None => return None,

                // If the last event is a `ConversationEvent`, we handle it.
                Some(InternalEvent::Event(_)) => {
                    self.last().map_or(self.base_config.clone(), |v| v.config)
                }

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

    #[must_use]
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = ConversationEventWithConfigRef<'_>> {
        Iter {
            stream: self,
            front_config: self.base_config.clone(),
            front: 0,
            back: self.events.len(),
        }
    }

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

pub struct IntoIter {
    current_config: PartialAppConfig,
    inner_iter: std::vec::IntoIter<InternalEvent>,
}

impl Iterator for IntoIter {
    type Item = ConversationEventWithConfig;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let event = self.inner_iter.next()?;

            match event {
                InternalEvent::ConfigDelta(delta) => {
                    if let Err(error) = self.current_config.merge(&(), delta.into_inner()) {
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
                        if let InternalEvent::ConfigDelta(ConfigDelta(delta)) = internal_event
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

struct Iter<'a> {
    stream: &'a ConversationStream,
    front_config: PartialAppConfig,
    front: usize,
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
                    if let Err(error) = self.front_config.merge(&(), delta.clone().into_inner()) {
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
                    && let Err(error) = config.merge(&(), delta.clone().into_inner())
                {
                    error!(%error, "Failed to merge config delta.");
                }
            }

            return Some(ConversationEventWithConfigRef { event, config });
        }

        None
    }
}

pub struct IterMut<'a> {
    iter: std::slice::IterMut<'a, InternalEvent>,
    front_config: PartialAppConfig,
}

impl<'a> Iterator for IterMut<'a> {
    type Item = ConversationEventWithConfigMut<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        for event in self.iter.by_ref() {
            match event {
                InternalEvent::ConfigDelta(delta) => {
                    if let Err(error) = self.front_config.merge(&(), delta.clone().into_inner()) {
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

#[derive(Debug, PartialEq, Clone)]
pub struct ConversationEventWithConfigRef<'a> {
    pub event: &'a ConversationEvent,
    pub config: PartialAppConfig,
}

#[derive(Debug, PartialEq)]
pub struct ConversationEventWithConfigMut<'a> {
    pub event: &'a mut ConversationEvent,
    pub config: PartialAppConfig,
}

#[derive(Debug, PartialEq, Clone)]
pub struct ConversationEventWithConfig {
    pub event: ConversationEvent,
    pub config: PartialAppConfig,
}

impl ConversationEventWithConfig {
    #[must_use]
    pub fn into_inner(self) -> ConversationEvent {
        self.event
    }

    #[must_use]
    pub fn into_kind(self) -> EventKind {
        self.event.kind
    }

    #[must_use]
    pub fn kind(&self) -> &EventKind {
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
        let (base_config, first_event) = events
            .next()
            .map_or((PartialAppConfig::empty(), None), |e| {
                (e.config, Some(e.event))
            });

        let mut stream = ConversationStream::new(base_config);
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

        stream.push(InternalEvent::ConfigDelta(ConfigDelta::new(
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
            InternalEvent::ConfigDelta(base_config) => Ok(ConversationStream {
                base_config: base_config.into_inner(),
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
