use std::{io, path::PathBuf};

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
    #[error("Config file not found: {0}")]
    MissingConfigFile(PathBuf),

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

    #[error("{0} not found: {1}")]
    NotFound(&'static str, String),

    #[error("IO error")]
    Io(#[from] io::Error),

    #[error("URL error")]
    Url(#[from] url::ParseError),

    #[error("Tool error")]
    Tool(#[from] jp_llm::ToolError),

    #[error("Bat error")]
    Bat(#[from] bat::error::Error),

    #[error("Attachment error: {0}")]
    Attachment(String),

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
}
