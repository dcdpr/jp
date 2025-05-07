use serde::Deserialize;
use serde_json::Value;
use time::OffsetDateTime;

use super::tool::ToolCall;

/// A chat completion response.
#[derive(Debug, Deserialize)]
pub struct ChatCompletion {
    /// The ID of the response.
    pub id: String,

    /// The name of the provider serving the response.
    ///
    /// This can differ from the model provider, e.g. if the model provider is
    /// Anthropic, but the provider server is Amazon AWS.
    pub provider: String,

    /// A list of "choices" made by the model in response to the prompt.
    ///
    /// Open AI supports requesting multiple "choices" (responses) from the
    /// model, but this is not currently supported by the Openrouter API.
    ///
    /// See: <https://github.com/OpenRouterTeam/openrouter-runner/issues/99>
    pub choices: Vec<Choice>,

    /// The time the response was created.
    #[serde(with = "time::serde::timestamp")]
    pub created: OffsetDateTime,

    /// The model used to generate the response.
    pub model: String,

    /// The object returned by the model (differs for streaming and
    /// non-streaming mode).
    pub object: ResponseObject,

    /// This fingerprint represents the backend configuration that the model
    /// runs with.
    ///
    /// Can be used in conjunction with the seed request parameter to understand
    /// when backend changes have been made that might impact determinism.
    ///
    /// Only present if the provider supports it.
    pub system_fingerprint: Option<String>,

    /// Usage statistics for the completion request.
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionError {
    pub error: ErrorResponse,

    #[serde(flatten)]
    _other: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged, variant_identifier)]
pub enum ResponseObject {
    #[serde(rename = "chat.completion")]
    ChatCompletion,
    #[serde(rename = "chat.completion.chunk")]
    ChatCompletionChunk,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Choice {
    NonStreaming(NonStreamingChoice),
    Streaming(StreamingChoice),
    NonChat(NonChatChoice),
}

impl Choice {
    #[must_use]
    pub fn content(&self) -> Option<&str> {
        match self {
            Self::NonStreaming(choice) => choice.message.content.as_deref(),
            Self::Streaming(choice) => choice.delta.content.as_deref(),
            Self::NonChat(choice) => Some(&choice.text),
        }
    }

    #[must_use]
    pub fn reasoning(&self) -> Option<&str> {
        match self {
            Self::NonStreaming(choice) => choice.message.reasoning.as_deref(),
            Self::Streaming(choice) => choice.delta.reasoning.as_deref(),
            Self::NonChat(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct NonStreamingChoice {
    pub finish_reason: FinishReason,
    pub native_finish_reason: String,
    pub message: NonStreamingMessage,
    pub error: Option<ErrorResponse>,

    // TODO: figure out what this is used for.
    pub index: usize,
}

/// The reason why the assistant stopped generating tokens.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename = "snake_case", rename_all = "snake_case")]
pub enum FinishReason {
    /// The assistant has finished requesting a tool call execution.
    ToolCalls,

    /// The assistant has stopped generating tokens.
    Stop,

    /// The assistant has reached the maximum length of accepted tokens.
    Length,

    /// The assistant has filtered out the content due to a flag from content
    /// filters.
    ContentFilter,

    /// The assistant encountered an error generating the response.
    Error,

    /// Undefined/unknown finish reason.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct NonStreamingMessage {
    pub role: String,
    pub content: Option<String>,
    pub reasoning: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct StreamingChoice {
    /// Similar to [`NonStreamingChoice::finish_reason`], but can be `None` if
    /// the stream is not finished.
    pub finish_reason: Option<FinishReason>,

    /// Similar to [`NonStreamingChoice::native_finish_reason`], but can be
    /// `None` if the stream is not finished.
    pub native_finish_reason: Option<String>,
    pub delta: StreamingDelta,
    pub error: Option<ErrorResponse>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct StreamingDelta {
    pub role: Option<String>,
    pub content: Option<String>,
    pub reasoning: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct NonChatChoice {
    pub finish_reason: FinishReason,
    pub text: String,
    pub error: Option<ErrorResponse>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CompletionChunk {
    pub id: String,
    pub choices: Vec<StreamingChoice>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Usage {
    pub completion_tokens: u32,
    pub completion_tokens_details: Option<CompletionTokensDetails>,
    pub cost: Option<f32>,
    pub prompt_tokens: u32,
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PromptTokensDetails {
    pub cached_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CompletionTokensDetails {
    pub reasoning_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ErrorResponse {
    pub code: u16,
    pub message: String,
    pub metadata: Option<ErrorMetadata>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum ErrorMetadata {
    /// Model provider flagged the input for moderation.
    Moderation {
        /// Why the input was flagged.
        reasons: Vec<String>,

        /// The text segment that was flagged, limited to 100 characters. If the
        /// flagged input is longer than 100 characters, it will be truncated in
        /// the middle and replaced with ...
        flagged_input: String,

        /// The name of the provider that requested moderation.
        provider_name: String,

        /// The slug of the model that was used.
        model_slug: String,
    },

    /// If the model provider encounters an error, the error will contain
    /// information about the issue.
    Provider {
        /// The name of the provider that encountered the error.
        provider_name: String,

        /// The raw error from the provider.
        raw: Value,
    },
}

/// API error codes returned by the Openrouter API.
#[derive(Debug, Clone, PartialEq)]
pub enum ErrorCode {
    BadRequest,
    InvalidCredentials,
    InsufficientCredits,
    ModerationInputFlagged,
    RequestTimeout,
    RateLimited,
    InvalidModelResponse,
    UnmatchedRoutingRequirements,
    Other(usize),
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        usize::deserialize(deserializer).map(Into::into)
    }
}

impl From<usize> for ErrorCode {
    fn from(code: usize) -> Self {
        match code {
            400 => ErrorCode::BadRequest,
            401 => ErrorCode::InvalidCredentials,
            402 => ErrorCode::InsufficientCredits,
            403 => ErrorCode::ModerationInputFlagged,
            408 => ErrorCode::RequestTimeout,
            429 => ErrorCode::RateLimited,
            502 => ErrorCode::InvalidModelResponse,
            503 => ErrorCode::UnmatchedRoutingRequirements,
            n => ErrorCode::Other(n),
        }
    }
}
