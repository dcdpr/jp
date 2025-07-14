//! Defines the Conversation structure.

use std::{fmt, str::FromStr};

use jp_config::{Partial, PartialConfig};
use jp_id::{
    parts::{GlobalId, TargetId, Variant},
    Id, NANOSECONDS_PER_DECISECOND,
};
use serde::{ser::SerializeStruct as _, Deserialize, Serialize};
use time::UtcDateTime;

use crate::error::{Error, Result};

/// A sequence of messages between the user and LLM.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Conversation {
    /// The optional title of the conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// The last time the conversation was activated.
    pub last_activated_at: UtcDateTime,

    /// Whether the conversation is stored in the user or workspace storage.
    #[serde(skip)]
    pub user: bool,

    /// The partial configuration persisted for this conversation.
    #[serde(default = "PartialConfig::empty")]
    config: PartialConfig,
}

impl Serialize for Conversation {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::Error as _;

        let mut n = 3;
        if self.title.is_some() {
            n += 1;
        }

        let mut state = serializer.serialize_struct("Conversation", n)?;

        if let Some(title) = &self.title {
            state.serialize_field("title", title)?;
        }

        state.serialize_field("last_activated_at", &self.last_activated_at)?;
        state.serialize_field("local", &self.user)?;

        state.serialize_field(
            "config",
            &trim_config(self.config.clone()).map_err(S::Error::custom)?,
        )?;

        state.end()
    }
}

impl Default for Conversation {
    fn default() -> Self {
        Self {
            last_activated_at: UtcDateTime::now(),
            title: None,
            config: PartialConfig::default_values(),
            user: false,
        }
    }
}

impl Conversation {
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            ..Default::default()
        }
    }

    #[must_use]
    pub fn with_local(mut self, local: bool) -> Self {
        self.user = local;
        self
    }

    #[must_use]
    pub fn config(&self) -> &PartialConfig {
        &self.config
    }

    #[must_use]
    pub fn config_mut(&mut self) -> &mut PartialConfig {
        &mut self.config
    }

    pub fn set_config(&mut self, config: PartialConfig) {
        self.config = config;
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

fn trim_config(
    cfg: jp_config::PartialConfig,
) -> std::result::Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    Ok(match serde_json::to_value(cfg)? {
        serde_json::Value::Object(v) => v
            .into_iter()
            .filter_map(|(k, v)| compact_value(v).map(|v| (k, v)))
            .collect::<serde_json::Map<String, serde_json::Value>>()
            .into(),
        v => v,
    })
}

fn compact_value(v: serde_json::Value) -> Option<serde_json::Value> {
    match v {
        serde_json::Value::Object(v) => {
            let map = v
                .into_iter()
                .filter_map(|(k, v)| compact_value(v).map(|v| (k, v)))
                .collect::<serde_json::Map<_, _>>();

            (!map.is_empty()).then(|| map.into())
        }
        serde_json::Value::Array(v) => {
            let vec = v.into_iter().filter_map(compact_value).collect::<Vec<_>>();

            (!vec.is_empty()).then(|| vec.into())
        }
        serde_json::Value::Null => None,
        _ => Some(v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversation_serialization() {
        let mut config = PartialConfig::default_values();
        config.assistant.provider.openai.base_url = None;
        config.assistant.provider.openrouter.app_name = None;
        config.style.code = <_>::empty();

        let conv = Conversation {
            title: None,
            last_activated_at: UtcDateTime::new(
                time::Date::from_calendar_date(2023, time::Month::January, 1).unwrap(),
                time::Time::MIDNIGHT,
            ),
            user: true,
            config,
        };

        insta::assert_json_snapshot!(conv);
    }
}
