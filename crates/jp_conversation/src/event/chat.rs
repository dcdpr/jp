//! See [`ChatRequest`].

use std::{fmt, ops};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A chat request event - the user's query or message.
///
/// This represents the user's side of a conversation turn.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatRequest {
    /// The user's query or message content.
    pub content: String,

    /// Optional JSON schema constraining the assistant's response format.
    ///
    /// When present, providers set their native structured output
    /// configuration and the assistant's response is emitted as
    /// `ChatResponse::Structured` instead of `ChatResponse::Message`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Map<String, Value>>,
}

impl From<String> for ChatRequest {
    fn from(content: String) -> Self {
        Self {
            content,
            schema: None,
        }
    }
}

impl From<&str> for ChatRequest {
    fn from(content: &str) -> Self {
        Self {
            content: content.to_owned(),
            schema: None,
        }
    }
}

impl fmt::Display for ChatRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.content.fmt(f)
    }
}

impl ops::Deref for ChatRequest {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.content
    }
}

impl ops::DerefMut for ChatRequest {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.content
    }
}

/// A chat response event - the assistant's response to a chat request.
///
/// Multiple `ChatResponse` events can be emitted for a single `ChatRequest`,
/// for example when the assistant first outputs reasoning, then the actual
/// response message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, rename_all = "snake_case")]
pub enum ChatResponse {
    /// A standard message response.
    Message {
        /// The message content.
        message: String,
    },

    /// Reasoning/thinking response that is not necessarily relevant to the
    /// final response.
    Reasoning {
        /// The reasoning content.
        reasoning: String,
    },

    /// Structured JSON response conforming to the schema from the
    /// preceding `ChatRequest`.
    Structured {
        /// The structured JSON value.
        ///
        /// After flush, this is the parsed JSON (object, array, etc.).
        /// During streaming, individual parts carry `Value::String`
        /// chunks that are concatenated by the `EventBuilder`.
        data: Value,
    },
}

impl ChatResponse {
    /// Creates a new message response.
    #[must_use]
    pub fn message(content: impl Into<String>) -> Self {
        Self::Message {
            message: content.into(),
        }
    }

    /// Creates a new reasoning response.
    #[must_use]
    pub fn reasoning(content: impl Into<String>) -> Self {
        Self::Reasoning {
            reasoning: content.into(),
        }
    }

    /// Creates a new structured response.
    #[must_use]
    pub fn structured(data: impl Into<Value>) -> Self {
        Self::Structured { data: data.into() }
    }

    /// Returns `true` if the response is a message.
    #[must_use]
    pub const fn is_message(&self) -> bool {
        matches!(self, Self::Message { .. })
    }

    /// Returns `true` if the response is reasoning.
    #[must_use]
    pub const fn is_reasoning(&self) -> bool {
        matches!(self, Self::Reasoning { .. })
    }

    /// Returns `true` if the response is structured data.
    #[must_use]
    pub const fn is_structured(&self) -> bool {
        matches!(self, Self::Structured { .. })
    }

    /// Returns a reference to the structured JSON data, if applicable.
    #[must_use]
    pub const fn as_structured_data(&self) -> Option<&Value> {
        match self {
            Self::Structured { data } => Some(data),
            _ => None,
        }
    }

    /// Consumes the response and returns the structured JSON data, if
    /// applicable.
    #[must_use]
    pub fn into_structured_data(self) -> Option<Value> {
        match self {
            Self::Structured { data } => Some(data),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "chat_tests.rs"]
mod tests;
