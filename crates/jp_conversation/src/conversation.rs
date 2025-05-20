//! Defines the Conversation structure.

use std::{fmt, str::FromStr};

use jp_id::{
    parts::{GlobalId, TargetId, Variant},
    Id, NANOSECONDS_PER_DECISECOND,
};
use serde::{Deserialize, Serialize};
use time::UtcDateTime;

use crate::{
    context::Context,
    error::{Error, Result},
};

/// A sequence of messages between the user and LLM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conversation {
    pub last_activated_at: UtcDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub context: Context,

    /// Whether the conversation is stored locally or in the workspace.
    #[serde(skip)]
    pub local: bool,
}

impl Default for Conversation {
    fn default() -> Self {
        Self {
            last_activated_at: UtcDateTime::now(),
            title: None,
            context: Context::default(),
            local: false,
        }
    }
}

impl Conversation {
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            last_activated_at: UtcDateTime::now(),
            title: Some(title.into()),
            context: Context::default(),
            local: false,
        }
    }
}

/// ID wrapper for Conversation
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ConversationId(#[serde(with = "jp_id::serde")] UtcDateTime);

impl fmt::Debug for ConversationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ConversationId")
            .field(&self.to_string())
            .finish()
    }
}

impl ConversationId {
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

    pub fn from_dirname(dirname: impl AsRef<str>) -> Result<Self> {
        dirname
            .as_ref()
            .split('-')
            .next()
            .ok_or(jp_id::Error::MissingTargetId.into())
            .and_then(Self::try_from_deciseconds_str)
    }

    pub fn to_dirname(&self, title: Option<&str>) -> Result<String> {
        let len = title.map(str::len).unwrap_or_default();
        let title = title
            .unwrap_or_default()
            .trim()
            .chars()
            .take(60)
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .trim_end_matches('-')
            .to_lowercase();

        let ts = self.as_deciseconds().to_string();
        if title.is_empty() {
            return Ok(ts);
        }

        let mut title = format!("{ts}-{title}");
        if len > 60
            && let Some(i) = title.rfind('-')
        {
            title.truncate(i);
        }

        Ok(title)
    }
}

impl TryFrom<UtcDateTime> for ConversationId {
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

impl Id for ConversationId {
    fn variant() -> Variant {
        'c'.into()
    }

    fn target_id(&self) -> TargetId {
        self.as_deciseconds().to_string().into()
    }

    fn global_id(&self) -> GlobalId {
        jp_id::global::get().into()
    }
}

impl fmt::Display for ConversationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.format_id(f)
    }
}

impl FromStr for ConversationId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        jp_id::parse::<Self>(s)
            .map(|p| p.target_id)
            .map_err(Into::into)
            .and_then(Self::try_from_deciseconds_str)
    }
}

impl Default for ConversationId {
    fn default() -> Self {
        Self::try_from(UtcDateTime::now()).expect("valid timestamp")
    }
}

/// Holds metadata about all conversations, like the current active
/// conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationsMetadata {
    /// The ID of the currently active conversation.
    ///
    /// If no active conversation exists, one is created.
    pub active_conversation_id: ConversationId,
}

impl ConversationsMetadata {
    #[must_use]
    pub fn new(active_conversation_id: ConversationId) -> Self {
        Self {
            active_conversation_id,
        }
    }
}

impl Default for ConversationsMetadata {
    fn default() -> Self {
        Self::new(ConversationId::default())
    }
}
