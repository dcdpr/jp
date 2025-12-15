//! See [`ChatRequest`].

use std::{fmt, ops};

use serde::{Deserialize, Serialize};

/// A chat request event - the user's query or message.
///
/// This represents the user's side of a conversation turn.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatRequest {
    /// The user's query or message content
    pub content: String,
}

impl From<String> for ChatRequest {
    fn from(content: String) -> Self {
        Self { content }
    }
}

impl From<&str> for ChatRequest {
    fn from(content: &str) -> Self {
        Self {
            content: content.to_owned(),
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
    /// A standard message response
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

    /// Returns the content of the response, either the message or the
    /// reasoning.
    #[must_use]
    pub fn content(&self) -> &str {
        match self {
            Self::Message { message, .. } => message,
            Self::Reasoning { reasoning, .. } => reasoning,
        }
    }

    /// Returns a mutable reference to the content of the response, either the
    /// message or the reasoning.
    #[must_use]
    pub const fn content_mut(&mut self) -> &mut String {
        match self {
            Self::Message { message, .. } => message,
            Self::Reasoning { reasoning, .. } => reasoning,
        }
    }

    /// Consumes the response and returns the content, either the message or
    /// the reasoning.
    #[must_use]
    pub fn into_content(self) -> String {
        match self {
            Self::Message { message, .. } => message,
            Self::Reasoning { reasoning, .. } => reasoning,
        }
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
}
