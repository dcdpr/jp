use std::path::PathBuf;

use crate::id::McpServerId;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Service error: {0}")]
    Service(#[from] rmcp::ServiceError),

    #[error("MCP error: {0}")]
    Mcp(#[from] rmcp::Error),

    #[error("Timeout error: {0}")]
    Timeout(#[from] tokio::time::error::Elapsed),

    #[error("Unknown tool: {0}")]
    UnknownTool(String),

    #[error("Unknown MCP server: {0}")]
    UnknownServer(McpServerId),

    #[error("Invalid tool choice: {0}, must be one of [auto, none, required, fn:<name>]")]
    UnknownToolChoice(String),

    #[error("Duplicate tool configured: {0}")]
    DuplicateTool(String),

    #[error("Missing environment variable: {0}")]
    MissingEnv(#[from] std::env::VarError),

    #[error("Checksum mismatch for server: {server} ({}), expected {expected}, got {got}", path.display())]
    ChecksumMismatch {
        server: String,
        path: PathBuf,
        expected: String,
        got: String,
    },

    #[error("Cannot spawn process: {cmd}, error: {error}")]
    CannotSpawnProcess {
        cmd: String,
        #[source]
        error: std::io::Error,
    },

    #[error("Process error: {cmd}, error: {error}")]
    ProcessError {
        cmd: String,
        #[source]
        error: std::io::Error,
    },

    #[error("Cannot read file: {path}, error: {error}")]
    CannotReadFile {
        path: std::path::PathBuf,
        #[source]
        error: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("Cannot locate binary: {path}, error: {error}")]
    CannotLocateBinary {
        path: std::path::PathBuf,
        #[source]
        error: Box<dyn std::error::Error + Send + Sync>,
    },
}

#[cfg(test)]
impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        if std::mem::discriminant(self) != std::mem::discriminant(other) {
            return false;
        }

        // Good enough for testing purposes
        format!("{self:?}") == format!("{other:?}")
    }
}
