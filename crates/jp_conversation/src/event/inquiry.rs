//! See [`InquiryRequest`] and [`InquiryResponse`].

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::Value;

/// Opaque identifier for an inquiry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InquiryId(String);

impl InquiryId {
    /// Creates a new inquiry ID.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns the ID as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InquiryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for InquiryId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for InquiryId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

/// An inquiry request event - requesting additional input or clarification.
///
/// This event can be triggered by tools, the assistant, or even the user, when
/// additional information is needed before proceeding.
/// The system should pause execution and wait for a corresponding
/// `InquiryResponse` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InquiryRequest {
    /// Identifier for this inquiry.
    ///
    /// This must match the `id` in the corresponding `InquiryResponse`.
    /// The caller determines the ID convention.
    pub id: InquiryId,

    /// The source of the inquiry (who is asking).
    pub source: InquirySource,

    /// The question being asked.
    pub question: InquiryQuestion,
}

impl InquiryRequest {
    /// Creates a new inquiry request.
    #[must_use]
    pub fn new(id: impl Into<InquiryId>, source: InquirySource, question: InquiryQuestion) -> Self {
        Self {
            id: id.into(),
            source,
            question,
        }
    }
}

/// The source/origin of an inquiry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum InquirySource {
    /// The inquiry originates from a tool execution.
    Tool {
        /// The name of the tool making the inquiry.
        name: String,
    },

    /// The inquiry originates from the assistant.
    Assistant,

    /// The inquiry originates from the user.
    User,

    /// The inquiry originates from another source.
    Other {
        /// The name or identifier of the source.
        name: String,
    },
}

impl InquirySource {
    /// Create a new inquiry source from a tool name.
    #[must_use]
    pub fn tool(name: impl Into<String>) -> Self {
        Self::Tool { name: name.into() }
    }

    /// Create a new inquiry source from an other source.
    #[must_use]
    pub fn other(name: impl Into<String>) -> Self {
        Self::Other { name: name.into() }
    }
}

/// A question requiring an answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InquiryQuestion {
    /// The question text to display.
    pub text: String,

    /// The type of answer expected.
    pub answer_type: InquiryAnswerType,

    /// Optional default answer.
    ///
    /// This can be used as a fallback in non-interactive environments or as a
    /// suggested default in interactive prompts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

impl InquiryQuestion {
    /// Creates a new inquiry question.
    #[must_use]
    pub const fn new(text: String, answer_type: InquiryAnswerType) -> Self {
        Self {
            text,
            answer_type,
            default: None,
        }
    }

    /// Sets the default value for the question.
    #[must_use]
    pub fn with_default(mut self, default: Value) -> Self {
        self.default = Some(default);
        self
    }

    /// Creates a new boolean inquiry question.
    #[must_use]
    pub const fn boolean(text: String) -> Self {
        Self::new(text, InquiryAnswerType::Boolean)
    }

    /// Creates a new select inquiry question from `SelectOption`s.
    #[must_use]
    pub const fn select(text: String, options: Vec<SelectOption>) -> Self {
        Self::new(text, InquiryAnswerType::Select { options })
    }

    /// Creates a new select inquiry question from plain values (no
    /// descriptions).
    #[must_use]
    pub fn select_values(text: String, values: impl IntoIterator<Item = Value>) -> Self {
        let options = values.into_iter().map(SelectOption::from).collect();
        Self::new(text, InquiryAnswerType::Select { options })
    }

    /// Creates a new text inquiry question.
    #[must_use]
    pub const fn text(text: String) -> Self {
        Self::new(text, InquiryAnswerType::Text)
    }
}

/// The type of answer expected for an inquiry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InquiryAnswerType {
    /// Boolean yes/no question.
    Boolean,

    /// Select from predefined options.
    Select {
        /// The available options to choose from.
        options: Vec<SelectOption>,
    },

    /// Free-form text input.
    Text,

    /// Free-form text input whose answer is not persisted on disk.
    Secret,
}

/// A single option in a select inquiry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectOption {
    /// The value returned when this option is selected.
    pub value: Value,

    /// Human-readable description of this option, used as help text in
    /// interactive prompts and as context for assistant-targeted inquiries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl SelectOption {
    /// Creates a new select option with a value and description.
    #[must_use]
    pub fn new(value: impl Into<Value>, description: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            description: Some(description.into()),
        }
    }
}

impl From<Value> for SelectOption {
    fn from(value: Value) -> Self {
        Self {
            value,
            description: None,
        }
    }
}

impl From<&str> for SelectOption {
    fn from(s: &str) -> Self {
        Self {
            value: s.into(),
            description: None,
        }
    }
}

/// The outcome of an [`InquiryRequest`].
///
/// This event MUST be in response to an `InquiryRequest` event, with a matching
/// `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum InquiryResponse {
    /// The inquiry was answered.
    Answered {
        /// ID matching the corresponding `InquiryRequest`.
        id: InquiryId,

        /// The answer provided.
        ///
        /// The shape of this value depends on the `answer_type` of the
        /// corresponding inquiry:
        ///
        /// - `Boolean`: `Value::Bool`
        /// - `Select`: one of the option values
        /// - `Text`: `Value::String`
        answer: Value,
    },

    /// The inquiry was closed without an answer.
    Cancelled {
        /// ID matching the corresponding `InquiryRequest`.
        id: InquiryId,

        /// Why the inquiry was closed without an answer.
        reason: CancellationReason,
    },

    /// The inquiry was answered, but the answer was deliberately not persisted
    /// (e.g. a secret value).
    ///
    /// The tool still received the answer in-memory; only the on-disk record
    /// omits it.
    Redacted {
        /// ID matching the corresponding `InquiryRequest`.
        id: InquiryId,
    },
}

impl InquiryResponse {
    /// Creates an answered inquiry response.
    #[must_use]
    pub fn answered(id: impl Into<InquiryId>, answer: Value) -> Self {
        Self::Answered {
            id: id.into(),
            answer,
        }
    }

    /// Creates an answered boolean inquiry response.
    #[must_use]
    pub fn boolean(id: impl Into<InquiryId>, answer: bool) -> Self {
        Self::answered(id, Value::Bool(answer))
    }

    /// Creates an answered select inquiry response.
    #[must_use]
    pub fn select(id: impl Into<InquiryId>, answer: impl Into<Value>) -> Self {
        Self::answered(id, answer.into())
    }

    /// Creates an answered text inquiry response.
    #[must_use]
    pub fn text(id: impl Into<InquiryId>, answer: String) -> Self {
        Self::answered(id, Value::String(answer))
    }

    /// The inquiry ID, present on every variant.
    #[must_use]
    pub const fn id(&self) -> &InquiryId {
        match self {
            Self::Answered { id, .. } | Self::Cancelled { id, .. } | Self::Redacted { id } => id,
        }
    }

    /// The answer provided, if this response carries one.
    ///
    /// Returns `None` for `Cancelled` and `Redacted` responses.
    #[must_use]
    pub const fn answer(&self) -> Option<&Value> {
        match self {
            Self::Answered { answer, .. } => Some(answer),
            Self::Cancelled { .. } | Self::Redacted { .. } => None,
        }
    }
}

impl<'de> Deserialize<'de> for InquiryResponse {
    /// Accepts both the tagged 082+ form and the legacy pre-082 flat form.
    ///
    /// The flat form (`{ "id", "answer" }`, no `outcome`) deserializes as
    /// `Answered`.
    /// A tagged `cancelled` event with no `reason` defaults to
    /// `CancellationReason::User`.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            id: InquiryId,
            #[serde(default)]
            outcome: Option<String>,
            #[serde(default)]
            answer: Option<Value>,
            #[serde(default)]
            reason: Option<CancellationReason>,
        }

        let raw = Raw::deserialize(deserializer)?;
        match raw.outcome.as_deref() {
            Some("answered") => {
                let answer = raw
                    .answer
                    .ok_or_else(|| de::Error::missing_field("answer"))?;
                Ok(Self::Answered { id: raw.id, answer })
            }
            Some("cancelled") => Ok(Self::Cancelled {
                id: raw.id,
                reason: raw.reason.unwrap_or(CancellationReason::User),
            }),
            Some("redacted") => Ok(Self::Redacted { id: raw.id }),
            Some(other) => Err(de::Error::unknown_variant(other, &[
                "answered",
                "cancelled",
                "redacted",
            ])),
            // Legacy pre-082 flat form: `{ "id", "answer" }` is an answer.
            None => {
                let answer = raw.answer.ok_or_else(|| {
                    de::Error::custom("inquiry response missing `outcome` and `answer`")
                })?;
                Ok(Self::Answered { id: raw.id, answer })
            }
        }
    }
}

/// Why an inquiry was closed without an answer.
///
/// Recorded on [`InquiryResponse::Cancelled`] for the audit trail only; it does
/// not drive retry behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancellationReason {
    /// The user explicitly cancelled (e.g. Ctrl-C at the prompt).
    User,

    /// The routing backend (prompter or inquiry backend) returned an error
    /// instead of an answer.
    BackendError,

    /// A question that requires a human answer could not be routed because no
    /// interactive terminal is available.
    NoPromptBackend,

    /// A question that requires a human answer was targeted at the assistant
    /// and refused to route to the inquiry backend.
    AssistantRoutingDenied,

    /// A reason produced by a newer JP that named a variant this build does not
    /// recognize.
    ///
    /// The payload is the unparsed serde tag, preserved verbatim so it
    /// round-trips unchanged.
    /// Audit-trail only; readers MUST NOT branch on the contents.
    Unknown(String),
}

impl CancellationReason {
    /// The serde tag for this reason.
    ///
    /// `Unknown` returns its preserved tag verbatim.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::User => "user",
            Self::BackendError => "backend_error",
            Self::NoPromptBackend => "no_prompt_backend",
            Self::AssistantRoutingDenied => "assistant_routing_denied",
            Self::Unknown(tag) => tag,
        }
    }

    /// Parses a serde tag into a reason, mapping unrecognized tags to
    /// [`Self::Unknown`].
    fn from_tag(tag: &str) -> Self {
        match tag {
            "user" => Self::User,
            "backend_error" => Self::BackendError,
            "no_prompt_backend" => Self::NoPromptBackend,
            "assistant_routing_denied" => Self::AssistantRoutingDenied,
            other => Self::Unknown(other.to_owned()),
        }
    }
}

impl Serialize for CancellationReason {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CancellationReason {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let tag = String::deserialize(deserializer)?;
        Ok(Self::from_tag(&tag))
    }
}

#[cfg(test)]
#[path = "inquiry_tests.rs"]
mod tests;
