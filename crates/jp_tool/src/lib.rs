use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod access;
pub use access::{
    AccessPolicy, Capability, EnvRule, FsAccessError, FsRule, NetRule,
    canonicalize_workspace_target, lexical_workspace_relative,
};

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

    /// Returns the content of the outcome if it is a success, panicking if it
    /// is not.
    ///
    /// # Panics
    ///
    /// Panics if the outcome is not a success.
    #[must_use]
    pub fn unwrap_content(self) -> String {
        self.into_content().unwrap()
    }
}

/// A request for additional input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Question {
    /// The question ID.
    ///
    /// This must be passed back to the tool when answering the question.
    pub id: String,

    /// The question to ask.
    ///
    /// This MUST be a single line of text for it to be displayed correctly.
    pub text: String,

    /// An optional preamble to display before the question.
    pub pre_amble: Option<String>,

    /// Type of answer expected
    pub answer_type: AnswerType,

    /// Optional default answer when no answer is provided.
    ///
    /// This can be used to select a default option when the question is
    /// presented to the user, or to use as the answer in non-interactive mode
    /// when no answer can be provided interactively.
    pub default: Option<Value>,
}

impl Question {
    /// Create a new text question.
    pub fn text(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            pre_amble: None,
            answer_type: AnswerType::Text,
            default: None,
        }
    }

    /// Create a new boolean question.
    pub fn boolean(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            pre_amble: None,
            answer_type: AnswerType::Boolean,
            default: None,
        }
    }

    /// Create a new boolean question.
    pub fn select(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            pre_amble: None,
            answer_type: AnswerType::Select { options: vec![] },
            default: None,
        }
    }

    /// Set the preamble text.
    #[must_use]
    pub fn with_preamble(mut self, pre_amble: impl Into<String>) -> Self {
        self.pre_amble = Some(pre_amble.into());
        self
    }

    /// Set the default answer.
    #[must_use]
    pub fn with_default(mut self, default: impl Into<Value>) -> Self {
        self.default = Some(default.into());
        self
    }

    /// Set the answer type.
    #[must_use]
    pub fn with_answer_type(mut self, answer_type: AnswerType) -> Self {
        self.answer_type = answer_type;
        self
    }

    /// Set the answer type to a select type with the given options.
    #[must_use]
    pub fn with_options(mut self, options: Vec<String>) -> Self {
        self.answer_type = AnswerType::Select { options };
        self
    }

    /// Add an option to the select answer type.
    ///
    /// Converts the answer type to a select type if it is not already.
    #[must_use]
    pub fn with_option(mut self, option: impl Into<String>) -> Self {
        match &mut self.answer_type {
            AnswerType::Select { options } => options.push(option.into()),
            _ => {
                self.answer_type = AnswerType::Select {
                    options: vec![option.into()],
                }
            }
        }

        self
    }
}

/// The type of answer expected for a given question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    pub root: Utf8PathBuf,

    // The action that the tool is being run for.
    pub action: Action,

    /// Access grants for this tool invocation.
    ///
    /// When `None`, the tool has unrestricted (but still workspace-confined)
    /// filesystem access.
    /// When `Some` with a non-empty `fs` list, only explicitly granted
    /// capabilities are available.
    #[serde(default)]
    pub access: Option<AccessPolicy>,
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

/// The action that a tool is being run for.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Run the tool.
    Run,

    /// Format the provided tool call arguments.
    FormatArguments,
}

impl Action {
    /// Returns whether the action is a run action.
    #[must_use]
    pub const fn is_run(&self) -> bool {
        matches!(self, Self::Run)
    }

    /// Returns whether the action is a format arguments action.
    #[must_use]
    pub const fn is_format_arguments(&self) -> bool {
        matches!(self, Self::FormatArguments)
    }
}
