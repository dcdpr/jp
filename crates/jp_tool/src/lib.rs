use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The result of a tool call.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outcome {
    /// The tool succeeded and produced content.
    Success { content: String },

    /// The tool failed with an error.
    Error {
        /// The error message.
        message: String,

        /// The error trace.
        trace: Vec<String>,

        /// Whether the error is transient and can be retried.
        transient: bool,
    },

    /// The tool requires additional input before it can complete the request.
    NeedsInput { question: Question },
}

impl Outcome {
    #[must_use]
    pub fn error(error: &(dyn std::error::Error + Send + Sync)) -> Self {
        Self::error_with_transient(error, true)
    }

    #[must_use]
    pub fn fail(error: &(dyn std::error::Error + Send + Sync)) -> Self {
        Self::error_with_transient(error, false)
    }

    #[must_use]
    pub fn error_with_transient(
        error: &(dyn std::error::Error + Send + Sync),
        transient: bool,
    ) -> Self {
        let message = error.to_string();
        let mut trace = vec![];
        let mut source = error.source();
        while let Some(error) = source {
            trace.push(format!("{error:#}"));
            source = error.source();
        }

        Outcome::Error {
            message,
            trace,
            transient,
        }
    }

    /// Returns the content of the outcome if it is a success.
    #[must_use]
    pub fn into_content(self) -> Option<String> {
        match self {
            Outcome::Success { content } => Some(content),
            Outcome::NeedsInput { .. } | Outcome::Error { .. } => None,
        }
    }
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

    /// Indicates a request to format tool call arguments, instead of running
    /// the tool.
    #[serde(default, skip_serializing_if = "is_false")]
    pub format_parameters: bool,
}

#[expect(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
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

/// How long to remember an answer to a question.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PersistLevel {
    /// Don't remember (just this once).
    None,

    /// Remember for this turn (all tool calls in this LLM interaction).
    Turn,
}
