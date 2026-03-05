use serde::Serialize;
use serde_json::Value;

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

    /// Response format for structured output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
}

/// Response format for structured output in the chat completions API.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    /// JSON schema response format.
    JsonSchema {
        /// The schema configuration.
        json_schema: JsonSchemaFormat,
    },
}

/// Schema definition for structured JSON output.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JsonSchemaFormat {
    /// Name for the schema.
    pub name: String,
    /// The JSON schema.
    pub schema: Value,
    /// Whether to enforce strict schema adherence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Default)]
pub struct Reasoning {
    pub exclude: bool,
    pub effort: ReasoningEffort,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    XHigh,
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
