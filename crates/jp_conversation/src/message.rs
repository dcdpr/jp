//! Defines the Message structure.

use std::{collections::BTreeMap, fmt, str::FromStr};

use jp_config::{PartialAppConfig, PartialConfig as _, model::id::ProviderId};
use jp_id::{
    Id, NANOSECONDS_PER_DECISECOND,
    parts::{GlobalId, TargetId, Variant},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::UtcDateTime;
use tracing::warn;

use crate::error::{Error, Result};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Messages(Vec<MessagePairWithConfig>);

impl Messages {
    #[must_use]
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &MessagePair> {
        self.0.iter().map(|m| &m.pair)
    }

    #[expect(clippy::should_implement_trait)]
    #[must_use]
    pub fn into_iter(self) -> impl DoubleEndedIterator<Item = MessagePair> {
        self.0.into_iter().map(|m| m.pair)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Removes the last message from the list, or [`None`] if the list is
    /// empty.
    pub fn pop(&mut self) -> Option<MessagePair> {
        self.0.pop().map(|m| m.pair)
    }

    /// Removes the first message from the list, or [`None`] if the list is
    /// empty.
    pub fn pop_front(&mut self) -> Option<MessagePair> {
        if self.0.is_empty() {
            return None;
        }

        Some(self.0.remove(0).pair)
    }

    /// Adds a message to the list.
    pub fn push(&mut self, pair: MessagePair, config: Option<PartialAppConfig>) {
        let config_delta = self
            .config()
            .delta(config.unwrap_or_else(PartialAppConfig::empty));

        self.0.push(MessagePairWithConfig { pair, config_delta });
    }

    pub fn extend(&mut self, other: Messages) {
        self.0.extend(other.0);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn as_ref(&self) -> MessagesRef<'_> {
        MessagesRef(self.0.as_slice())
    }

    #[must_use]
    pub fn config(&self) -> PartialAppConfig {
        self.as_ref().config()
    }

    pub fn set_config(&mut self, next: PartialAppConfig) {
        let config_delta = self.config().delta(next);

        if let Some(v) = self.0.last_mut() {
            v.config_delta = config_delta;
        }
    }
}

impl From<Vec<MessagePair>> for Messages {
    fn from(v: Vec<MessagePair>) -> Self {
        Self(v.into_iter().map(MessagePairWithConfig::from).collect())
    }
}

impl From<Vec<MessagePairWithConfig>> for Messages {
    fn from(v: Vec<MessagePairWithConfig>) -> Self {
        Self(v)
    }
}

#[derive(Debug, Clone, Default)]
pub struct MessagesRef<'a>(&'a [MessagePairWithConfig]);

impl MessagesRef<'_> {
    pub fn iter(&self) -> impl Iterator<Item = &MessagePairWithConfig> {
        self.0.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn last(&self) -> Option<&MessagePair> {
        self.0.last().map(|m| &m.pair)
    }

    #[must_use]
    pub fn to_messages(&self) -> Messages {
        Messages(self.0.to_vec())
    }

    #[must_use]
    pub fn config(&self) -> PartialAppConfig {
        self.0
            .iter()
            .map(|m| m.config_delta.clone())
            .reduce(|mut a, b| {
                if let Err(error) = a.merge(&(), b) {
                    warn!(?error, "Failed to merge configuration partial.");
                }

                a
            })
            .unwrap_or_else(PartialAppConfig::empty)
    }
}

/// A message pair with the configuration delta.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagePairWithConfig {
    /// The message pair.
    #[serde(flatten)]
    pair: MessagePair,

    /// The delta of the configuration, relative to the previous message. The
    /// first message in the list contains a full copy of the configuration at
    /// the time of the message.
    ///
    /// If the config delta is empty, it means that the configuration has not
    /// changed since the last message.
    #[serde(default, skip_serializing_if = "PartialAppConfig::is_empty")]
    config_delta: PartialAppConfig,
}

impl std::ops::Deref for MessagePairWithConfig {
    type Target = MessagePair;

    fn deref(&self) -> &Self::Target {
        &self.pair
    }
}

impl From<(MessagePair, PartialAppConfig)> for MessagePairWithConfig {
    fn from((pair, config_delta): (MessagePair, PartialAppConfig)) -> Self {
        Self { pair, config_delta }
    }
}

impl From<MessagePair> for MessagePairWithConfig {
    fn from(pair: MessagePair) -> Self {
        (pair, PartialAppConfig::empty()).into()
    }
}

/// A single exchange between user and LLM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessagePair {
    /// The timestamp of the message pair.
    pub timestamp: UtcDateTime,

    /// The user message that was sent.
    pub message: UserMessage,

    /// The assistant message that was replied to the user.
    pub reply: AssistantMessage,
}

impl MessagePair {
    /// Creates a new message pair with the current timestamp.
    #[must_use]
    pub fn new(message: UserMessage, reply: AssistantMessage) -> Self {
        Self {
            timestamp: UtcDateTime::now(),
            message,
            reply,
        }
    }

    #[must_use]
    pub fn with_message(mut self, message: impl Into<UserMessage>) -> Self {
        self.message = message.into();
        self
    }

    #[must_use]
    pub fn with_reasoning(mut self, reasoning: impl Into<String>) -> Self {
        self.reply.reasoning = Some(reasoning.into());
        self
    }

    #[must_use]
    pub fn attach_metadata(mut self, key: impl Into<String>, metadata: impl Into<Value>) -> Self {
        self.reply.metadata.insert(key.into(), metadata.into());
        self
    }

    #[must_use]
    pub fn with_reply(mut self, reply: impl Into<AssistantMessage>) -> Self {
        self.reply = reply.into();
        self
    }

    #[must_use]
    pub fn split(self) -> (UserMessage, AssistantMessage) {
        (self.message, self.reply)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, rename_all = "snake_case")]
pub enum UserMessage {
    Query(String),
    ToolCallResults(Vec<ToolCallResult>),
}

impl UserMessage {
    #[must_use]
    pub fn query(&self) -> Option<&str> {
        match self {
            Self::Query(query) if !query.is_empty() => Some(query),
            _ => None,
        }
    }

    #[must_use]
    pub fn tool_call_results(&self) -> &[ToolCallResult] {
        match self {
            Self::ToolCallResults(results) if !results.is_empty() => results,
            _ => &[],
        }
    }

    #[must_use]
    pub fn as_query_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::Query(query) => Some(query),
            Self::ToolCallResults(_) => None,
        }
    }
}

impl From<String> for UserMessage {
    fn from(message: String) -> Self {
        Self::Query(message)
    }
}

impl From<&str> for UserMessage {
    fn from(message: &str) -> Self {
        Self::Query(message.to_string())
    }
}

impl From<Vec<ToolCallResult>> for UserMessage {
    fn from(results: Vec<ToolCallResult>) -> Self {
        Self::ToolCallResults(results)
    }
}

impl Default for UserMessage {
    fn default() -> Self {
        Self::Query(String::new())
    }
}

// TODO: An assistant message cannot be empty, so we should model this type such
// that either `content` or `tool_calls` is present, or both, with optional
// `reasoning`.
//
// E.g. make `Content` an enum, containing either `Text`, `ToolCalls` or
// `TextWithToolCalls`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantMessage {
    /// The provider that produced the message.
    pub provider: ProviderId,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallRequest>,

    /// Opaque provider-specific metadata.
    ///
    /// The shape of this data depends on the provider and model.
    ///
    /// For example, for Openai , we use this to store the opaque reasoning data
    /// which is a JSON object in the shape of:
    ///
    /// ```json
    /// {
    ///   "id": "...",
    ///   "summary": [
    ///     {
    ///       "text": "...",
    ///       "type": "summary_text"
    ///     }
    ///   ],
    ///   "type": "reasoning",
    ///   "encrypted_content": "...",
    ///   "status": "..."
    /// }
    /// ```
    ///
    /// For Openai, this data is expected to be returned as-is when generating a
    /// request to the API. For other providers, the behavior might be
    /// different.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
}

impl AssistantMessage {
    #[must_use]
    pub fn new(provider: ProviderId) -> Self {
        Self {
            provider,
            metadata: BTreeMap::default(),
            reasoning: None,
            content: None,
            tool_calls: vec![],
        }
    }
}

impl<T: Into<String>> From<(ProviderId, T)> for AssistantMessage {
    fn from((provider, message): (ProviderId, T)) -> Self {
        Self {
            provider,
            metadata: BTreeMap::default(),
            reasoning: None,
            content: Some(message.into()),
            tool_calls: vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub id: String,
    pub name: String,
    #[serde(with = "jp_serde::repr::base64_json_map")]
    pub arguments: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub id: String,
    #[serde(with = "jp_serde::repr::base64_string")]
    pub content: String,
    pub error: bool,
}

/// ID wrapper for Message
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MessageId(#[serde(with = "jp_id::serde")] UtcDateTime);

impl MessageId {
    #[must_use]
    pub fn timestamp(&self) -> UtcDateTime {
        self.0
    }

    #[must_use]
    pub fn as_deciseconds(&self) -> i128 {
        self.timestamp().unix_timestamp_nanos() / i128::from(NANOSECONDS_PER_DECISECOND)
    }

    pub fn try_from_deciseconds(deciseconds: i128) -> Result<Self> {
        let timestamp = UtcDateTime::from_unix_timestamp_nanos(
            deciseconds * i128::from(NANOSECONDS_PER_DECISECOND),
        )
        .map_err(|e| jp_id::Error::InvalidTimestamp(e.to_string()))?;

        Ok(Self(timestamp))
    }

    pub fn try_from_deciseconds_str(deciseconds: impl AsRef<str>) -> Result<Self> {
        let deciseconds = deciseconds.as_ref().parse::<i128>().map_err(|_| {
            Error::InvalidIdFormat(format!("Invalid deciseconds: {}", deciseconds.as_ref()))
        })?;

        Self::try_from_deciseconds(deciseconds)
    }
}

impl TryFrom<UtcDateTime> for MessageId {
    type Error = Error;

    fn try_from(timestamp: UtcDateTime) -> Result<Self> {
        let nanos = timestamp.nanosecond();
        let truncated_nanos = (nanos / NANOSECONDS_PER_DECISECOND) * NANOSECONDS_PER_DECISECOND;

        timestamp
            .replace_nanosecond(truncated_nanos)
            .map_err(|e| jp_id::Error::InvalidTimestamp(e.to_string()).into())
            .map(Self)
    }
}

impl Id for MessageId {
    fn variant() -> Variant {
        'e'.into()
    }

    fn target_id(&self) -> TargetId {
        self.as_deciseconds().to_string().into()
    }

    fn global_id(&self) -> GlobalId {
        jp_id::global::get().into()
    }
}

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.format_id(f)
    }
}

impl FromStr for MessageId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        jp_id::parse::<Self>(s)
            .map(|p| p.target_id)
            .map_err(Into::into)
            .and_then(Self::try_from_deciseconds_str)
    }
}

impl Default for MessageId {
    fn default() -> Self {
        Self::try_from(UtcDateTime::now()).expect("valid timestamp")
    }
}
