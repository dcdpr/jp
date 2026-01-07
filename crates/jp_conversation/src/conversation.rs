//! Defines the Conversation structure.

use std::{fmt, str::FromStr};

use jp_id::{
    Id, NANOSECONDS_PER_DECISECOND,
    parts::{GlobalId, TargetId, Variant},
};
use jp_serde::skip_if;
use serde::{Deserialize, Serialize};
use time::UtcDateTime;

use crate::error::{Error, Result};

/// A sequence of events between the user and LLM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    /// The optional title of the conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// The last time the conversation was activated.
    pub last_activated_at: UtcDateTime,

    /// Whether the conversation is stored in the user or workspace storage.
    // TODO: rename to `user_local`
    #[serde(default, rename = "local", skip_serializing_if = "skip_if::is_false")]
    pub user: bool,

    /// When the conversation expires.
    ///
    /// An expired conversation that is not active, may be garbage collected by
    /// the system.
    ///
    /// The expiration timestamp is the *earliest* time at which the
    /// conversation will be garbage collected. In other words, if the timestamp
    /// is in the future, garbage collection will not occur, if the timestamp is
    /// *exactly* now, the conversation *might* be garbage collected, but it
    /// might also happen at a later time, when the timestamp is in the past.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<UtcDateTime>,

    /// The time of the last event, or `None` if the conversation is empty.
    #[serde(skip)]
    pub last_event_at: Option<UtcDateTime>,

    /// The number of events in the conversation.
    #[serde(skip)]
    pub events_count: usize,
}

impl Default for Conversation {
    fn default() -> Self {
        Self {
            last_activated_at: UtcDateTime::now(),
            title: None,
            user: false,
            expires_at: None,
            last_event_at: None,
            events_count: 0,
        }
    }
}

impl Conversation {
    /// Creates a new conversation with the given title.
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            ..Default::default()
        }
    }

    /// Sets whether the conversation is local.
    #[must_use]
    pub const fn with_local(mut self, local: bool) -> Self {
        self.user = local;
        self
    }

    /// Sets whether the conversation is ephemeral.
    #[must_use]
    pub const fn with_ephemeral(mut self, ephemeral: Option<UtcDateTime>) -> Self {
        self.expires_at = ephemeral;
        self
    }

    /// Sets the last activated at timestamp.
    #[must_use]
    pub const fn with_last_activated_at(mut self, last_activated_at: UtcDateTime) -> Self {
        self.last_activated_at = last_activated_at;
        self
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
    /// Get the timestamp of the conversation id.
    #[must_use]
    pub const fn timestamp(&self) -> UtcDateTime {
        self.0
    }

    /// Get the timestamp of the conversation id as deciseconds.
    #[must_use]
    pub fn as_deciseconds(&self) -> i128 {
        self.timestamp().unix_timestamp_nanos() / i128::from(NANOSECONDS_PER_DECISECOND)
    }

    /// Try to create a conversation id from deciseconds.
    ///
    /// # Errors
    ///
    /// Returns an error if the deciseconds cannot be converted to a valid UTC
    /// timestamp.
    pub fn try_from_deciseconds(deciseconds: i128) -> Result<Self> {
        let timestamp = UtcDateTime::from_unix_timestamp_nanos(
            deciseconds * i128::from(NANOSECONDS_PER_DECISECOND),
        )
        .map_err(|e| jp_id::Error::InvalidTimestamp(e.to_string()))?;

        Ok(Self(timestamp))
    }

    /// Try to create a conversation id from a string of deciseconds.
    ///
    /// # Errors
    ///
    /// Returns an error if the deciseconds cannot be parsed or converted to a
    /// valid UTC timestamp.
    pub fn try_from_deciseconds_str(deciseconds: impl AsRef<str>) -> Result<Self> {
        let deciseconds = deciseconds.as_ref().parse::<i128>().map_err(|_| {
            Error::InvalidIdFormat(format!("Invalid deciseconds: {}", deciseconds.as_ref()))
        })?;

        Self::try_from_deciseconds(deciseconds)
    }

    /// Create a conversation id from a directory name.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory name is missing the target ID or
    /// timestamp.
    pub fn try_from_dirname(dirname: impl AsRef<str>) -> Result<Self> {
        dirname
            .as_ref()
            .split('-')
            .next()
            .ok_or_else(|| jp_id::Error::MissingTargetId.into())
            .and_then(Self::try_from_deciseconds_str)
    }

    /// Create a directory name from the conversation id.
    pub fn to_dirname(&self, title: Option<&str>) -> String {
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
            return ts;
        }

        let mut title = format!("{ts}-{title}");
        if len > 60
            && let Some(i) = title.rfind('-')
        {
            title.truncate(i);
        }

        title
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationsMetadata {
    /// The ID of the currently active conversation.
    ///
    /// If no active conversation exists, one is created.
    pub active_conversation_id: ConversationId,
}

impl ConversationsMetadata {
    /// Creates a new conversations metadata.
    #[must_use]
    pub const fn new(active_conversation_id: ConversationId) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversation_serialization() {
        let conv = Conversation {
            title: None,
            last_activated_at: UtcDateTime::new(
                time::Date::from_calendar_date(2023, time::Month::January, 1).unwrap(),
                time::Time::MIDNIGHT,
            ),
            user: true,
            expires_at: None,
            last_event_at: None,
            events_count: 0,
        };

        insta::assert_json_snapshot!(conv);
    }
}
