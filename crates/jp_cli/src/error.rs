use std::io;

use crate::cmd;

pub(crate) type Result<T> = std::result::Result<T, Error>;

/// CLI Error types
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Command error: {0}")]
    Command(#[from] cmd::Error),

    #[error("Config error: {0}")]
    Config(#[from] jp_config::Error),

    #[error("Workspace error: {0}")]
    Workspace(#[from] jp_workspace::Error),

    #[error("Conversation error: {0}")]
    Conversation(#[from] jp_conversation::Error),

    #[error("MCP error: {0}")]
    Mcp(#[from] jp_mcp::Error),

    #[error("LLM error: {0}")]
    Llm(#[from] jp_llm::Error),

    #[error("{0} not found: {1}")]
    NotFound(&'static str, String),

    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("URL error: {0}")]
    Url(#[from] url::ParseError),

    #[error("Bat error: {0}")]
    Bat(#[from] bat::error::Error),

    #[error("Attachment error: {0}")]
    Attachment(String),

    #[error("Editor error: {0}")]
    Editor(String),

    #[error("Task error: {0}")]
    Task(Box<dyn std::error::Error + Send + Sync>),
}
