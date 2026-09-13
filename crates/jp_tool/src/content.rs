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

use serde::{Deserialize, Serialize};
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

    /// Whether the tool reported success or failure.
    pub status: ToolStatus,

    /// Structured data supplied alongside the ordered content.
    pub structured_content: Option<Value>,

    /// Opaque protocol metadata, preserved across forwarding.
    pub metadata: Option<Map<String, Value>>,
}

impl ToolResult {
    /// Whether the result reports a tool failure rather than a service failure.
    #[must_use]
    pub fn is_error(&self) -> bool {
        matches!(self.status, ToolStatus::Error(_))
    }

    /// Failure details supplied by the tool, if any.
    #[must_use]
    pub fn error_details(&self) -> Option<&ErrorDetails> {
        match &self.status {
            ToolStatus::Error(error) => Some(error),
            ToolStatus::Success | ToolStatus::Unspecified => None,
        }
    }

    /// A successful result carrying one text block.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::text(text)],
            status: ToolStatus::Success,
            structured_content: None,
            metadata: None,
        }
    }

    /// A failed result carrying one text block and no further detail.
    #[must_use]
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![ContentBlock::text(text)],
            status: ToolStatus::Error(ErrorDetails::default()),
            structured_content: None,
            metadata: None,
        }
    }

    /// The first input request in the content, when the tool is asking for one.
    #[must_use]
    pub fn input_request(&self) -> Option<&InputRequest> {
        self.content.iter().find_map(|block| match block {
            ContentBlock::Question(request) => Some(request),
            _ => None,
        })
    }

    /// Flatten the content to the text a provider receives.
    ///
    /// Text blocks and the text side of resources are joined with a blank line,
    /// in content order; a binary resource contributes its URI, since its bytes
    /// are not text.
    /// Error metadata is not appended to the tool's content.
    ///
    /// Callers that render blocks themselves should read [`content`] instead.
    ///
    /// [`content`]: Self::content
    #[must_use]
    pub fn to_text(&self) -> String {
        self.content
            .iter()
            .filter_map(ContentBlock::as_text)
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// The status reported by a tool, with failure details attached only to errors.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolStatus {
    /// The upstream protocol omitted its optional status field.
    Unspecified,
    /// The tool explicitly reported success.
    Success,
    /// The tool reported failure, with optional details.
    Error(ErrorDetails),
}

/// Detail a tool attached to a failure.
///
/// Arrives as `_meta["computer.jp/error"]` on an MCP-shaped result.
/// A failure without it is non-transient with no trace.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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

        /// Opaque protocol metadata for this text block.
        metadata: Option<Map<String, Value>>,
    },

    /// A resource the tool produced or read.
    Resource(Resource),

    /// Base64-encoded image content.
    Image(ImageContent),

    /// Base64-encoded audio content.
    Audio {
        /// Base64-encoded audio bytes.
        data: String,
        /// Media type of the audio data.
        mime_type: String,
        /// Audience, priority, and modification time.
        annotations: Option<Annotations>,
    },

    /// A resource reference without embedded content.
    ResourceLink(ResourceLink),

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
            metadata: None,
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
                ResourceContent::Blob(_) | ResourceContent::EncodedBlob(_) => Some(&resource.uri),
            },
            Self::Question(_) | Self::Image(_) | Self::Audio { .. } | Self::ResourceLink(_) => None,
        }
    }
}

/// A resource, identified by URI and carrying its content.
///
/// Embedded resource content and its protocol metadata remain separate from
/// optional presentation information.
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
    /// Available to rendering consumers; raw-content projections leave it out.
    pub formatted: Option<String>,

    /// Opaque metadata on the enclosing content block.
    pub metadata: Option<Map<String, Value>>,

    /// Opaque metadata on the embedded resource itself.
    pub content_metadata: Option<Map<String, Value>>,
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
            metadata: None,
            content_metadata: None,
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

    /// Base64 data received from MCP, retained without rewriting its encoding.
    EncodedBlob(String),
}

/// An image block, retaining its encoded bytes, media type, and metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageContent {
    /// Base64-encoded data as supplied by the tool.
    pub data: String,
    /// Media type of the encoded data.
    pub mime_type: String,
    /// Audience, priority, and modification time.
    pub annotations: Option<Annotations>,
    /// Opaque protocol metadata.
    pub metadata: Option<Map<String, Value>>,
}

/// An MCP resource reference without embedded content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceLink {
    /// URI identifying the resource.
    pub uri: String,
    /// Machine-readable resource name.
    pub name: String,
    /// Display title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Resource media type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Declared resource size in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u32>,
    /// Resource icons and their presentation hints.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icons: Option<Vec<ResourceIcon>>,
    /// Audience, priority, and modification time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Annotations>,
    /// Opaque protocol metadata.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
}

/// An icon supplied with a resource reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceIcon {
    /// URI of the icon, including data URIs.
    pub src: String,
    /// Icon media type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Declared image dimensions, using MCP's size notation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sizes: Option<Vec<String>>,
    /// Background theme for which the icon is intended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<IconTheme>,
}

/// Background theme of a resource icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IconTheme {
    Light,
    Dark,
}

/// MCP annotations on a block or resource.
///
/// Carried so a result that arrives with them can be handed back out intact.
/// Nothing in JP reads them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Annotations {
    /// Who the content is meant for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audience: Option<Vec<Role>>,

    /// How important the content is, from `0.0` to `1.0`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<f64>,

    /// When the content last changed, as an ISO 8601 timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
}

/// A party in the conversation, as MCP names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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

impl From<Result<String, String>> for ToolResult {
    fn from(result: Result<String, String>) -> Self {
        match result {
            Ok(text) => Self::text(text),
            Err(text) => Self::error(text),
        }
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
                content: vec![ContentBlock::text(if trace.is_empty() {
                    message
                } else {
                    format!("{message}\n\nTrace:\n{}", trace.join("\n"))
                })],
                status: ToolStatus::Error(ErrorDetails { transient, trace }),
                structured_content: None,
                metadata: None,
            },
            Outcome::NeedsInput { mut question } => {
                let mut content: Vec<_> = question
                    .pre_amble
                    .take()
                    .into_iter()
                    .map(ContentBlock::text)
                    .collect();
                content.push(ContentBlock::Question(question.into()));
                Self {
                    content,
                    status: ToolStatus::Success,
                    structured_content: None,
                    metadata: None,
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "content_tests.rs"]
mod tests;
