use serde::Serialize;

use super::{
    chat::{self, Transform},
    tool::{self, Tool, ToolChoice},
};

/// Chat completion request matching the `OpenRouter` API schema.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct ChatCompletion {
    /// The model ID to use.
    pub model: String,

    /// The list of messages.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<RequestMessage>,

    /// Reasoning configuration.
    ///
    /// Should be `None` if the model does not support reasoning.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<Reasoning>,

    /// Tool calling field.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,

    /// Tool choice field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,

    /// Message transforms.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub transforms: Vec<Transform>,

    /// Stop words.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,

    /// Whether to return log probabilities of the output tokens or not. If
    /// true, returns the log probabilities of each output token returned in the
    /// content of message.
    #[serde(skip_serializing_if = "is_false")]
    pub logprobs: bool,

    /// Whether to include usage statistics in the response.
    ///
    /// Enabling usage accounting will add a few hundred milliseconds to the
    /// last response as the API calculates token counts and costs. This only
    /// affects the final message and does not impact overall streaming
    /// performance.
    ///
    /// Default is `false`.
    #[serde(skip_serializing_if = "is_false")]
    pub usage: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Default)]
pub struct Reasoning {
    pub exclude: bool,
    pub effort: ReasoningEffort,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    XHigh,
    High,
    #[default]
    Medium,
    Low,
}

#[expect(clippy::trivially_copy_pass_by_ref)]
fn is_false(v: &bool) -> bool {
    !v
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
