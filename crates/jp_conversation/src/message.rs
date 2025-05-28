//! Defines the Message structure.

use std::{collections::BTreeMap, fmt, str::FromStr};

use jp_id::{
    parts::{GlobalId, TargetId, Variant},
    Id, NANOSECONDS_PER_DECISECOND,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::UtcDateTime;

use crate::{
    context::Context,
    error::{Error, Result},
};

/// A single exchange between user and LLM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessagePair {
    /// The timestamp of the message pair.
    pub timestamp: UtcDateTime,

    /// The user message that was sent.
    pub message: UserMessage,

    /// The assistant message that was replied to the user.
    pub reply: AssistantMessage,

    /// The context that was active when this message pair was generated.
    pub context: Context,
}

impl MessagePair {
    /// Creates a new message pair with the current timestamp.
    #[must_use]
    pub fn new(message: UserMessage, reply: AssistantMessage) -> Self {
        Self {
            timestamp: UtcDateTime::now(),
            message,
            reply,
            context: Context::default(),
        }
    }

    #[must_use]
    pub fn with_context(mut self, context: Context) -> Self {
        self.context = context;
        self
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

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AssistantMessage {
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

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallRequest>,
}

impl<T: Into<String>> From<T> for AssistantMessage {
    fn from(message: T) -> Self {
        Self {
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
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub id: String,
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
