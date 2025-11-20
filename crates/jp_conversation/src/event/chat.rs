use std::{collections::BTreeMap, fmt, ops};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A chat request event - the user's query or message.
///
/// This represents the user's side of a conversation turn.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged, rename_all = "snake_case")]
pub enum ChatResponse {
    /// A standard message response
    Message { message: String },

    /// Reasoning/thinking response that is not necessarily relevant to the
    /// final response.
    Reasoning {
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
    #[must_use]
    pub fn content(&self) -> &str {
        match self {
            ChatResponse::Message { message, .. } => message,
            ChatResponse::Reasoning { reasoning, .. } => reasoning,
        }
    }

    #[must_use]
    pub fn into_content(self) -> String {
        match self {
            ChatResponse::Message { message, .. } => message,
            ChatResponse::Reasoning { reasoning, .. } => reasoning,
        }
    }

    #[must_use]
    pub fn metadata(&self) -> Option<&BTreeMap<String, Value>> {
        match self {
            ChatResponse::Message { .. } => None,
            ChatResponse::Reasoning { metadata, .. } => Some(metadata),
        }
    }
}

// pub struct ChatResponse {
//     /// The content of the response
//     pub content: String,
//
//     /// The type/variant of this response
//     #[serde(flatten)]
//     pub variant: ChatResponseVariant,
//
//     /// Opaque provider-specific metadata.
//     ///
//     /// The shape of this data depends on the provider and model.
//     ///
//     /// For example, for `OpenAI`, we use this to store the opaque reasoning
//     /// data which includes signatures to validate the authenticity of reasoning
//     /// content.
//     ///
//     /// The provider can be inferred by looking at the most recent `ConfigDelta`
//     /// event that precedes this response.
//     #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
//     pub metadata: BTreeMap<String, Value>,
// }

// /// The type of chat response.
// #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
// #[serde(tag = "variant", rename_all = "snake_case")]
// pub enum ChatResponseVariant {
//     /// A standard message response
//     Message,
//
//     /// Reasoning/thinking response that is not necessarily relevant to the
//     /// final response.
//     Reasoning,
// }

impl ChatResponse {
    #[must_use]
    pub fn message(content: impl Into<String>) -> Self {
        Self::Message {
            message: content.into(),
        }
    }

    #[must_use]
    pub fn reasoning(content: impl Into<String>) -> Self {
        Self::Reasoning {
            reasoning: content.into(),
            metadata: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_metadata(mut self, metadata: BTreeMap<String, Value>) -> Self {
        match &mut self {
            ChatResponse::Reasoning { metadata: m, .. } if !metadata.is_empty() => {
                m.extend(metadata);
            }
            _ => {}
        }

        self
    }

    #[must_use]
    pub fn is_message(&self) -> bool {
        matches!(self, ChatResponse::Message { .. })
    }

    #[must_use]
    pub fn is_reasoning(&self) -> bool {
        matches!(self, ChatResponse::Reasoning { .. })
    }
}
