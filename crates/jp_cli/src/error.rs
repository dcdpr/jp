use std::io;

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

    #[error("Model error")]
    Model(#[from] jp_model::Error),

    #[error("{0} not found: {1}")]
    NotFound(&'static str, String),

    #[error("IO error")]
    Io(#[from] io::Error),

    #[error("URL error")]
    Url(#[from] url::ParseError),

    #[error("Bat error")]
    Bat(#[from] bat::error::Error),

    #[error("Attachment error: {0}")]
    Attachment(String),

    #[error("Editor error: {0}")]
    Editor(String),

    #[error("Missing editor")]
    MissingEditor,

    #[error("No model configured. Use `--model` to specify a model.")]
    UndefinedModel,

    #[error("Task error: {0}")]
    Task(Box<dyn std::error::Error + Send + Sync>),

    #[error("Template error")]
    Template(#[from] minijinja::Error),

    #[error("Undefined template variable: {0}")]
    TemplateUndefinedVariable(String),

    #[error("Replay error: {0}")]
    Replay(String),

    #[error("JSON error")]
    Json(#[from] serde_json::Error),

    #[error("Invalid JSON schema: {0}")]
    Schema(String),

    #[error("Cannot locate binary: {0}")]
    Which(#[from] which::Error),

    #[error("Unknown model: {}", .model)]
    UnknownModel {
        model: String,
        available: Vec<String>,
    },
}
