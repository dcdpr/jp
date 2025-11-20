use serde::{Deserialize, Serialize};
use serde_json::Value;

/// An inquiry request event - requesting additional input or clarification.
///
/// This event can be triggered by tools, the assistant, or even the user,
/// when additional information is needed before proceeding. The system should
/// pause execution and wait for a corresponding `InquiryResponse` event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryRequest {
    /// Unique identifier for this inquiry.
    ///
    /// This must match the `id` in the corresponding `InquiryResponse`.
    pub id: String,

    /// The source of the inquiry (who is asking).
    pub source: InquirySource,

    /// The question being asked.
    pub question: InquiryQuestion,
}

impl InquiryRequest {
    #[must_use]
    pub fn new(id: String, source: InquirySource, question: InquiryQuestion) -> Self {
        Self {
            id,
            source,
            question,
        }
    }

    #[must_use]
    pub fn from_tool(id: String, tool_name: String, question: InquiryQuestion) -> Self {
        Self::new(id, InquirySource::Tool { name: tool_name }, question)
    }

    #[must_use]
    pub fn from_assistant(id: String, question: InquiryQuestion) -> Self {
        Self::new(id, InquirySource::Assistant, question)
    }

    #[must_use]
    pub fn from_user(id: String, question: InquiryQuestion) -> Self {
        Self::new(id, InquirySource::User, question)
    }
}

/// The source/origin of an inquiry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    #[must_use]
    pub fn new(text: String, answer_type: InquiryAnswerType) -> Self {
        Self {
            text,
            answer_type,
            default: None,
        }
    }

    #[must_use]
    pub fn with_default(mut self, default: Value) -> Self {
        self.default = Some(default);
        self
    }

    #[must_use]
    pub fn boolean(text: String) -> Self {
        Self::new(text, InquiryAnswerType::Boolean)
    }

    #[must_use]
    pub fn select(text: String, options: Vec<Value>) -> Self {
        Self::new(text, InquiryAnswerType::Select { options })
    }

    #[must_use]
    pub fn text(text: String) -> Self {
        Self::new(text, InquiryAnswerType::Text)
    }
}

/// The type of answer expected for an inquiry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InquiryAnswerType {
    /// Boolean yes/no question.
    Boolean,

    /// Select from predefined options.
    ///
    /// The options can be any JSON value (strings, numbers, booleans, etc.).
    Select {
        /// The available options to choose from.
        options: Vec<Value>,
    },

    /// Free-form text input.
    Text,
}

/// An inquiry response event - the answer to an inquiry request.
///
/// This event MUST be in response to an `InquiryRequest` event, with a
/// matching `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InquiryResponse {
    /// ID matching the corresponding `InquiryRequest`.
    pub id: String,

    /// The answer provided.
    ///
    /// The shape of this value depends on the `answer_type` of the
    /// corresponding inquiry:
    /// - `Boolean`: `Value::Bool`
    /// - `Select`: `Value::String` (one of the options)
    /// - `Text`: `Value::String`
    pub answer: Value,
}

impl InquiryResponse {
    #[must_use]
    pub fn new(id: String, answer: Value) -> Self {
        Self { id, answer }
    }

    #[must_use]
    pub fn boolean(id: String, answer: bool) -> Self {
        Self::new(id, Value::Bool(answer))
    }

    #[must_use]
    pub fn select(id: String, answer: impl Into<Value>) -> Self {
        Self::new(id, answer.into())
    }

    #[must_use]
    pub fn text(id: String, answer: String) -> Self {
        Self::new(id, Value::String(answer))
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        self.answer.as_bool()
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        self.answer.as_str()
    }

    #[must_use]
    pub fn as_string(&self) -> Option<String> {
        self.answer.as_str().map(ToString::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inquiry_request_serialization() {
        let request = InquiryRequest::from_tool(
            "test-id".to_string(),
            "file_editor".to_string(),
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
        let response = InquiryResponse::boolean("test-id".to_string(), true);

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["id"], "test-id");
        assert_eq!(json["answer"], true);

        let deserialized: InquiryResponse = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, response);
    }

    #[test]
    fn test_inquiry_question_types() {
        // Boolean
        let q = InquiryQuestion::boolean("Confirm?".to_string());
        assert!(matches!(q.answer_type, InquiryAnswerType::Boolean));

        // Select with strings
        let q = InquiryQuestion::select("Choose one:".to_string(), vec![
            "option1".into(),
            "option2".into(),
        ]);
        assert!(matches!(q.answer_type, InquiryAnswerType::Select { .. }));

        // Select with integers
        let q = InquiryQuestion::select("Choose a number:".to_string(), vec![
            Value::Number(1.into()),
            Value::Number(2.into()),
            Value::Number(3.into()),
        ]);
        if let InquiryAnswerType::Select { options } = q.answer_type {
            assert_eq!(options.len(), 3);
            assert_eq!(options[0], 1);
            assert_eq!(options[1], 2);
            assert_eq!(options[2], 3);
        } else {
            panic!("Expected Select variant");
        }

        // Text
        let q = InquiryQuestion::text("Enter name:".to_string());
        assert!(matches!(q.answer_type, InquiryAnswerType::Text));
    }

    #[test]
    fn test_inquiry_response_helpers() {
        let response = InquiryResponse::boolean("id".to_string(), true);
        assert_eq!(response.as_bool(), Some(true));
        assert_eq!(response.as_str(), None);

        let response = InquiryResponse::text("id".to_string(), "hello".to_string());
        assert_eq!(response.as_bool(), None);
        assert_eq!(response.as_str(), Some("hello"));
        assert_eq!(response.as_string(), Some("hello".to_string()));

        // Select with string
        let response = InquiryResponse::select("id".to_string(), "option1");
        assert_eq!(response.as_str(), Some("option1"));

        // Select with integer
        let response = InquiryResponse::select("id".to_string(), 42);
        assert_eq!(response.answer, 42);
    }
}
