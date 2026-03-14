//! See [`InquiryRequest`] and [`InquiryResponse`].

use std::fmt;

use serde::{Deserialize, Serialize};
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
/// additional information is needed before proceeding. The system should pause
/// execution and wait for a corresponding `InquiryResponse` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InquiryRequest {
    /// Identifier for this inquiry.
    ///
    /// This must match the `id` in the corresponding `InquiryResponse`. The
    /// caller determines the ID convention.
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
    /// This can be used as a fallback in non-interactive environments
    /// or as a suggested default in interactive prompts.
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

    /// Creates a new select inquiry question from plain values (no descriptions).
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

/// An inquiry response event - the answer to an inquiry request.
///
/// This event MUST be in response to an `InquiryRequest` event, with a matching
/// `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InquiryResponse {
    /// ID matching the corresponding `InquiryRequest`.
    pub id: InquiryId,

    /// The answer provided.
    ///
    /// The shape of this value depends on the `answer_type` of the
    /// corresponding inquiry:
    /// - `Boolean`: `Value::Bool`
    /// - `Select`: one of the option values
    /// - `Text`: `Value::String`
    pub answer: Value,
}

impl InquiryResponse {
    /// Creates a new inquiry response.
    #[must_use]
    pub fn new(id: impl Into<InquiryId>, answer: Value) -> Self {
        Self {
            id: id.into(),
            answer,
        }
    }

    /// Creates a new boolean inquiry response.
    #[must_use]
    pub fn boolean(id: impl Into<InquiryId>, answer: bool) -> Self {
        Self::new(id, Value::Bool(answer))
    }

    /// Creates a new select inquiry response.
    #[must_use]
    pub fn select(id: impl Into<InquiryId>, answer: impl Into<Value>) -> Self {
        Self::new(id, answer.into())
    }

    /// Creates a new text inquiry response.
    #[must_use]
    pub fn text(id: impl Into<InquiryId>, answer: String) -> Self {
        Self::new(id, Value::String(answer))
    }

    /// Returns the answer as a boolean, if applicable.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        self.answer.as_bool()
    }

    /// Returns the answer as a string, if applicable.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        self.answer.as_str()
    }

    /// Returns the answer as a string, if applicable.
    #[must_use]
    pub fn as_string(&self) -> Option<String> {
        self.answer.as_str().map(ToString::to_string)
    }
}

#[cfg(test)]
#[path = "inquiry_tests.rs"]
mod tests;
