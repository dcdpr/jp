use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The result of a tool call.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outcome {
    /// The tool succeeded and produced content.
    Success { content: String },

    /// The tool requires additional input before it can complete the request.
    NeedsInput { question: Question },
}

/// A request for additional input.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Question {
    /// The question ID.
    ///
    /// This must be passed back to the tool when answering the question.
    pub id: String,

    /// The question to ask.
    pub text: String,

    /// Type of answer expected
    pub answer_type: AnswerType,

    /// Optional default answer when no answer is provided.
    ///
    /// This can be used to select a default option when the question is
    /// presented to the user, or to use as the answer in non-interactive mode
    /// when no answer can be provided interactively.
    pub default: Option<Value>,
}

/// The type of answer expected for a given question.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub enum AnswerType {
    /// Boolean yes/no question
    Boolean,

    /// Select from predefined options
    Select { options: Vec<String> },

    /// Free-form text input
    Text,
}

/// Contextual information available to a tool.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Context {
    /// The root path that the tool should run in.
    pub root: PathBuf,
}

impl From<String> for Outcome {
    fn from(content: String) -> Self {
        Self::Success { content }
    }
}

impl From<&str> for Outcome {
    fn from(content: &str) -> Self {
        content.to_owned().into()
    }
}

impl From<Question> for Outcome {
    fn from(question: Question) -> Self {
        Self::NeedsInput { question }
    }
}
