use std::io;

use camino::Utf8PathBuf;
use jp_conversation::ConversationId;
use url::Url;

use crate::cmd;

pub(crate) type Result<T> = std::result::Result<T, Error>;

/// CLI Error types
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("Command error")]
    Command(#[from] cmd::Error),

    #[error("Configuration error")]
    Config(#[from] jp_config::Error),

    #[error("unable to load configuration file")]
    ConfigLoader(#[from] jp_config::fs::ConfigLoaderError),

    #[error(transparent)]
    KeyValue(#[from] jp_config::assignment::KvAssignmentError),

    /// Missing config file.
    #[error("Config file not found: {path}")]
    MissingConfigFile {
        path: Utf8PathBuf,
        searched: Vec<Utf8PathBuf>,
    },

    #[error("CLI Config error: {0}")]
    CliConfig(String),

    #[error("Workspace error")]
    Workspace(#[from] jp_workspace::Error),

    #[error("Conversation error")]
    Conversation(#[from] jp_conversation::Error),

    #[error("MCP error")]
    Mcp(#[from] jp_mcp::Error),

    #[error("LLM error")]
    Llm(#[from] jp_llm::Error),

    /// The inquiry model override (`conversation.inquiry.assistant.model`)
    /// could not be used.
    ///
    /// A dedicated variant so the failure is attributed to the override: the
    /// underlying LLM error (e.g. a missing API key environment variable)
    /// would otherwise be indistinguishable from a main-model failure and
    /// point the user at the wrong config.
    #[error("Inquiry model override '{model}' is unusable")]
    InquiryModelOverride { model: String, source: jp_llm::Error },

    #[error("{0} not found: {1}")]
    NotFound(&'static str, String),

    #[error("IO error")]
    Io(#[from] io::Error),

    #[error("URL error")]
    Url(#[from] url::ParseError),

    #[error("Tool error")]
    Tool(#[from] jp_llm::ToolError),

    #[error("Syntax highlighting error")]
    SyntaxHighlight(#[from] syntect::Error),

    #[error("Attachment error: {0}")]
    Attachment(String),

    /// An attachment handler failed while processing `uri`.
    ///
    /// Keeps the boxed source so its full cause chain can be rendered, and the
    /// URI so the user can see which attachment failed.
    #[error("Attachment error for {uri}")]
    AttachmentFailed {
        uri: Url,
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The conversation referenced by a `jp://` attachment has been archived or
    /// deleted from the workspace.
    /// Surfaced as its own variant so query-time loaders can warn and skip dead
    /// references rather than abort the whole query.
    #[error("Attachment conversation '{id}' not found")]
    AttachmentConversationMissing { id: ConversationId, uri: Url },

    #[error("Editor error: {0}")]
    Editor(String),

    #[error("Missing editor")]
    MissingEditor,

    #[error("Task error: {0}")]
    Task(Box<dyn std::error::Error + Send + Sync>),

    #[error("Template error")]
    Template(#[from] minijinja::Error),

    #[error("Undefined template variable: {0}")]
    TemplateUndefinedVariable(String),

    #[error("JSON error")]
    Json(#[from] serde_json::Error),

    #[error("TOML error")]
    Toml(#[from] toml::de::Error),

    #[error("Cannot locate binary: {0}")]
    Which(#[from] which::Error),

    #[error("Unknown model: {}", .model)]
    UnknownModel {
        model: String,
        available: Vec<String>,
    },

    #[error("Model ID error")]
    ModelId(#[from] jp_config::model::id::ModelIdConfigError),

    // TODO: we should not have this error variant.
    #[error("Failed to inquire")]
    Inquire(#[from] inquire::error::InquireError),

    #[error("Failed to write to buffer")]
    Fmt(#[from] std::fmt::Error),

    #[error("Invalid schema: {0}")]
    Schema(String),

    #[error("No structured data in the assistant's response")]
    MissingStructuredData,

    #[error("Timed out waiting for lock on conversation {0}")]
    LockTimeout(ConversationId),

    #[error("No conversation targeted")]
    NoConversationTarget,

    #[error("Cannot start a new conversation together with --fork, --replay, or --id")]
    NewConflictsWithTarget,

    /// The user requested conversation target help.
    #[error("target help")]
    TargetHelp { session: bool, multi: bool },

    #[error("Compaction error: {0}")]
    Compaction(String),
}
