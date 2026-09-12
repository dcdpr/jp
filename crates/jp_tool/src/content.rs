//! What a tool execution attempt produced, and what it is asking for.
//!
//! [`ToolResult`] is the ordered content one attempt returned, plus whether it
//! failed.
//! A [`ContentBlock`] is one piece of that content: text, a resource, or a
//! request for input.
//!
//! The shape follows MCP's, so a result that arrives from an MCP server carries
//! across without being flattened on the way in, and one JP assembles itself
//! can be handed back out.
//! Fields MCP defines and JP does not act on are carried as data rather than
//! dropped.
//!
//! Tools speaking the [`Outcome`] protocol are converted at the boundary; see
//! the `From` implementations below.

use serde_json::{Map, Value, json};

use crate::{AnswerType, Outcome, Question, QuestionId};

/// What one tool execution attempt produced.
///
/// An attempt that ends by asking for input is not a failure: its content
/// carries a [`ContentBlock::Question`], and the caller runs the tool again
/// once it has the answer.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    /// The blocks the tool produced, in the order it produced them.
    pub content: Vec<ContentBlock>,

    /// Whether the tool reported a failure.
    pub is_error: bool,

    /// Extra detail about a failure, when the tool supplied it.
    ///
    /// Always `None` when `is_error` is `false`.
    pub error: Option<ErrorDetails>,
}

impl ToolResult {
    /// A successful result carrying one text block.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::text(text)],
            is_error: false,
            error: None,
        }
    }

    /// A failed result carrying one text block and no further detail.
    #[must_use]
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::text(text)],
            is_error: true,
            error: Some(ErrorDetails::default()),
        }
    }

    /// The first input request in the content, when the tool is asking for one.
    #[must_use]
    pub fn input_request(&self) -> Option<&InputRequest> {
        self.content.iter().find_map(|block| match block {
            ContentBlock::Question(request) => Some(request),
            ContentBlock::Text { .. } | ContentBlock::Resource(_) => None,
        })
    }

    /// Flatten the content to the text a provider receives.
    ///
    /// Text blocks and the text side of resources are joined with a blank line,
    /// in content order; a binary resource contributes its URI, since its bytes
    /// are not text.
    /// An error's trace is appended after the message.
    ///
    /// Callers that render blocks themselves should read [`content`] instead.
    ///
    /// [`content`]: Self::content
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = self
            .content
            .iter()
            .filter_map(ContentBlock::as_text)
            .collect::<Vec<_>>()
            .join("\n\n");

        let trace = self
            .error
            .as_ref()
            .map(|error| error.trace.as_slice())
            .unwrap_or_default();

        if !trace.is_empty() {
            out.push_str(&format!("\n\nTrace:\n{}", trace.join("\n")));
        }

        out
    }
}

/// Detail a tool attached to a failure.
///
/// Arrives as `_meta["computer.jp/error"]` on an MCP-shaped result.
/// A failure without it is non-transient with no trace.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ErrorDetails {
    /// Whether running the tool again could succeed.
    pub transient: bool,

    /// The error's source chain, outermost first.
    pub trace: Vec<String>,
}

/// One piece of a tool's output.
#[derive(Debug, Clone, PartialEq)]
pub enum ContentBlock {
    /// Text for the model to read.
    Text {
        text: String,

        /// The format of the text, when the tool declared one.
        ///
        /// MCP tools never set it; `None` means plain text.
        mime_type: Option<String>,

        /// MCP annotations, carried but not acted on.
        annotations: Option<Annotations>,
    },

    /// A resource the tool produced or read.
    Resource(Resource),

    /// Input the tool needs before it can finish.
    Question(InputRequest),
}

impl ContentBlock {
    /// A plain text block.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            mime_type: None,
            annotations: None,
        }
    }

    /// The block's text, for a caller assembling a plain-text result.
    ///
    /// A resource contributes its text content, or its URI when the content is
    /// binary.
    /// A question contributes nothing: it is answered, not read.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text, .. } => Some(text),
            Self::Resource(resource) => match &resource.content {
                ResourceContent::Text(text) => Some(text),
                ResourceContent::Blob(_) => Some(&resource.uri),
            },
            Self::Question(_) => None,
        }
    }
}

/// A resource, identified by URI and carrying its content.
///
/// The first four fields are MCP's; the rest are JP's, and an MCP-sourced
/// resource leaves them empty.
#[derive(Debug, Clone, PartialEq)]
pub struct Resource {
    /// The URI identifying this resource.
    pub uri: String,

    /// The resource's content.
    pub content: ResourceContent,

    /// The content's media type, such as `text/rust` or `image/png`.
    pub mime_type: Option<String>,

    /// MCP annotations, carried but not acted on.
    pub annotations: Option<Annotations>,

    /// A short name for the resource.
    pub name: Option<String>,

    /// A human-readable title, falling back to `name` and then `uri`.
    pub title: Option<String>,

    /// What the resource is.
    pub description: Option<String>,

    /// Content already formatted for the model.
    ///
    /// When set, it is what the model sees; `content` remains the resource's
    /// actual bytes.
    pub formatted: Option<String>,
}

impl Resource {
    /// A text resource with no metadata beyond its URI.
    #[must_use]
    pub fn text(uri: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            content: ResourceContent::Text(text.into()),
            mime_type: None,
            annotations: None,
            name: None,
            title: None,
            description: None,
            formatted: None,
        }
    }
}

/// A resource's content, matching MCP's text-or-blob model.
#[derive(Debug, Clone, PartialEq)]
pub enum ResourceContent {
    /// UTF-8 text.
    Text(String),

    /// Bytes, such as an image or a PDF.
    Blob(Vec<u8>),
}

/// MCP annotations on a block or resource.
///
/// Carried so a result that arrives with them can be handed back out intact.
/// Nothing in JP reads them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Annotations {
    /// Who the content is meant for.
    pub audience: Vec<Role>,

    /// How important the content is, from `0.0` to `1.0`.
    pub priority: Option<f64>,

    /// When the content last changed, as an ISO 8601 timestamp.
    pub last_modified: Option<String>,
}

/// A party in the conversation, as MCP names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// Input a tool needs before it can finish.
///
/// Who answers is the host's decision, not the tool's: the same request can be
/// put to the user, answered from configuration, or sent to an assistant.
#[derive(Debug, Clone, PartialEq)]
pub struct InputRequest {
    /// Identifies the request, and keys the answer on re-execution.
    pub id: QuestionId,

    /// A one-line prompt shown beside the input.
    ///
    /// Supporting material belongs in the content blocks preceding this one.
    pub label: String,

    /// JSON Schema the answer must satisfy.
    pub schema: Map<String, Value>,

    /// The answer used when none is given.
    pub default: Option<Value>,

    /// Whether the answer must not be written to disk.
    ///
    /// A secret answer is not echoed while it is typed, and the recorded
    /// inquiry response holds a redaction marker rather than the answer.
    pub secret: bool,
}

impl From<Question> for InputRequest {
    fn from(question: Question) -> Self {
        let Question {
            id,
            text,
            pre_amble: _,
            answer_type,
            default,
        } = question;

        Self {
            id,
            label: text,
            secret: matches!(answer_type, AnswerType::Secret),
            schema: answer_type.to_schema(),
            default,
        }
    }
}

impl AnswerType {
    /// The JSON Schema an answer of this type must satisfy.
    ///
    /// A secret answer is a string like any other; that it must not be
    /// persisted is carried by [`InputRequest::secret`], not by the schema, so
    /// the rule cannot be lost by rewriting the schema.
    #[must_use]
    pub fn to_schema(&self) -> Map<String, Value> {
        let schema = match self {
            Self::Boolean => json!({ "type": "boolean" }),
            Self::Select { options } => json!({ "type": "string", "enum": options }),
            Self::Text | Self::Secret => json!({ "type": "string" }),
        };

        schema.as_object().cloned().unwrap_or_default()
    }
}

impl From<Outcome> for ToolResult {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Success { content } => Self::text(content),
            Outcome::Error {
                message,
                trace,
                transient,
            } => Self {
                content: vec![ContentBlock::text(message)],
                is_error: true,
                error: Some(ErrorDetails { transient, trace }),
            },
            Outcome::NeedsInput { question } => Self {
                content: vec![ContentBlock::Question(question.into())],
                is_error: false,
                error: None,
            },
        }
    }
}

#[cfg(test)]
#[path = "content_tests.rs"]
mod tests;
