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
/// This event can be triggered by tools, the assistant, or even the user,
/// when additional information is needed before proceeding. The system should
/// pause execution and wait for a corresponding `InquiryResponse` event.
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
/// This event MUST be in response to an `InquiryRequest` event, with a
/// matching `id`.
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
mod tests {
    use test_log::test;

    use super::*;

    #[test]
    fn test_inquiry_id_display() {
        let id = InquiryId::new("fs_modify_file.__permission__");
        assert_eq!(id.to_string(), "fs_modify_file.__permission__");
        assert_eq!(id.as_str(), "fs_modify_file.__permission__");
    }

    #[test]
    fn test_inquiry_id_equality_and_hash() {
        use std::collections::HashMap;

        let id1 = InquiryId::new("same");
        let id2 = InquiryId::new("same");
        let id3 = InquiryId::new("different");

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);

        let mut map = HashMap::new();
        map.insert(id1, "value");
        assert_eq!(map.get(&id2), Some(&"value"));
        assert_eq!(map.get(&id3), None);
    }

    #[test]
    fn test_inquiry_id_serialization() {
        let id = InquiryId::new("test-id");
        let json = serde_json::to_value(&id).unwrap();
        assert_eq!(json, "test-id"); // transparent serialization

        let deserialized: InquiryId = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, id);
    }

    #[test]
    fn test_inquiry_request_serialization() {
        let request = InquiryRequest::new(
            "test-id",
            InquirySource::Tool {
                name: "file_editor".to_string(),
            },
            InquiryQuestion::boolean("Do you want to proceed?".to_string())
                .with_default(Value::Bool(false)),
        );

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["id"], "test-id");
        assert_eq!(json["source"]["source"], "tool");
        assert_eq!(json["source"]["name"], "file_editor");
        assert_eq!(json["question"]["text"], "Do you want to proceed?");
        assert_eq!(json["question"]["answer_type"]["type"], "boolean");
        assert_eq!(json["question"]["default"], false);

        let deserialized: InquiryRequest = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, request);
    }

    #[test]
    fn test_inquiry_response_serialization() {
        let response = InquiryResponse::boolean("test-id", true);

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["id"], "test-id");
        assert_eq!(json["answer"], true);

        let deserialized: InquiryResponse = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, response);
    }

    #[test]
    fn test_inquiry_question_types() {
        let q = InquiryQuestion::boolean("Confirm?".to_string());
        assert!(matches!(q.answer_type, InquiryAnswerType::Boolean));

        let q = InquiryQuestion::select("Choose one:".to_string(), vec![
            SelectOption::new("y", "yes"),
            SelectOption::new("n", "no"),
        ]);
        if let InquiryAnswerType::Select { options } = &q.answer_type {
            assert_eq!(options.len(), 2);
            assert_eq!(options[0].value, "y");
            assert_eq!(options[1].value, "n");
            assert_eq!(options[0].description.as_deref(), Some("yes"));
            assert_eq!(options[1].description.as_deref(), Some("no"));
        } else {
            panic!("Expected Select variant");
        }

        let q = InquiryQuestion::select_values("Pick:".to_string(), vec![
            Value::Number(1.into()),
            Value::Number(2.into()),
        ]);
        if let InquiryAnswerType::Select { options } = &q.answer_type {
            assert_eq!(options.len(), 2);
            assert_eq!(options[0].value, 1);
            assert_eq!(options[1].value, 2);
            assert!(options[0].description.is_none());
            assert!(options[1].description.is_none());
        } else {
            panic!("Expected Select variant");
        }

        let q = InquiryQuestion::text("Enter name:".to_string());
        assert!(matches!(q.answer_type, InquiryAnswerType::Text));
    }

    #[test]
    fn test_select_option_serialization() {
        let opt = SelectOption::new("y", "Run tool");
        let json = serde_json::to_value(&opt).unwrap();
        assert_eq!(json["value"], "y");
        assert_eq!(json["description"], "Run tool");

        let opt_no_desc = SelectOption::from("n");
        let json = serde_json::to_value(&opt_no_desc).unwrap();
        assert_eq!(json["value"], "n");
        assert!(json.get("description").is_none());

        let deserialized: SelectOption = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, opt_no_desc);
    }

    #[test]
    fn test_inquiry_response_helpers() {
        let response = InquiryResponse::boolean("id", true);
        assert_eq!(response.as_bool(), Some(true));
        assert_eq!(response.as_str(), None);

        let response = InquiryResponse::text("id", "hello".to_string());
        assert_eq!(response.as_str(), Some("hello"));

        let response = InquiryResponse::select("id", "option1");
        assert_eq!(response.as_str(), Some("option1"));

        let response = InquiryResponse::select("id", 42);
        assert_eq!(response.answer, 42);
    }
}
