//! See [`ChatRequest`].

use std::{collections::BTreeMap, fmt, ops};

use serde::{Deserialize, Serialize};
use serde_json::Value;

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

        /// Opaque provider-specific reasoning metadata.
        ///
        /// The shape of this data depends on the provider and model.
        ///
        /// For example, for `OpenAI`, we use this to store the opaque reasoning
        /// data which includes signatures to validate the authenticity of
        /// reasoning content.
        ///
        /// The provider can be inferred by merging all `ConfigDelta` events
        /// that precede this response.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        metadata: BTreeMap<String, Value>,
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
            metadata: BTreeMap::new(),
        }
    }

    /// Attaches metadata to the response, if applicable.
    #[must_use]
    pub fn with_metadata(mut self, metadata: BTreeMap<String, Value>) -> Self {
        match &mut self {
            Self::Reasoning { metadata: m, .. } if !metadata.is_empty() => {
                m.extend(metadata);
            }
            _ => {}
        }

        self
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

    /// Consumes the response and returns the content, either the message or
    /// the reasoning.
    #[must_use]
    pub fn into_content(self) -> String {
        match self {
            Self::Message { message, .. } => message,
            Self::Reasoning { reasoning, .. } => reasoning,
        }
    }

    /// Returns the metadata of the response, if applicable.
    #[must_use]
    pub const fn metadata(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            Self::Message { .. } => None,
            Self::Reasoning { metadata, .. } => Some(metadata),
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
