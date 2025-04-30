use serde::Serialize;

use super::{
    chat::{self, Transform},
    tool::{self, Tool},
};

/// Chat completion request matching the `OpenRouter` API schema.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct ChatCompletion {
    /// The model ID to use.
    pub model: String,

    /// The list of messages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<RequestMessage>,

    /// Reasoning configuration.
    ///
    /// Should be `None` if the model does not support reasoning.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,

    /// Tool calling field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,

    /// Message transforms.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transforms: Vec<Transform>,

    /// Stop words.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,

    /// Whether to return log probabilities of the output tokens or not. If
    /// true, returns the log probabilities of each output token returned in the
    /// content of message.
    #[serde(default, skip_serializing_if = "logprobs_is_false")]
    pub logprobs: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Default)]
pub struct Reasoning {
    pub exclude: bool,
    pub effort: ReasoningEffort,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    High,
    #[default]
    Medium,
    Low,
}

#[expect(clippy::trivially_copy_pass_by_ref)]
fn logprobs_is_false(logprobs: &bool) -> bool {
    !logprobs
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "lowercase", tag = "role")]
pub enum RequestMessage {
    User(chat::Message),
    Assistant(chat::Message),
    System(chat::Message),
    Tool(tool::Message),
}

impl RequestMessage {
    #[must_use]
    pub fn content_mut(&mut self) -> &mut [chat::Content] {
        match self {
            Self::Assistant(m) | Self::User(m) | Self::System(m) => m.content.as_mut_slice(),
            Self::Tool(_) => &mut [],
        }
    }

    #[must_use]
    pub fn chat_message_mut(&mut self) -> Option<&mut chat::Message> {
        match self {
            Self::Assistant(m) | Self::User(m) | Self::System(m) => Some(m),
            Self::Tool(_) => None,
        }
    }
}
