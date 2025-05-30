use crate::config::McpServerId;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Service error: {0}")]
    Service(#[from] rmcp::ServiceError),

    #[error("MCP error: {0}")]
    Mcp(#[from] rmcp::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Timeout error: {0}")]
    Timeout(#[from] tokio::time::error::Elapsed),

    #[error("Unknown tool: {0}")]
    UnknownTool(String),

    #[error("Unknown MCP server: {0}")]
    UnknownServer(McpServerId),

    #[error("Invalid tool choice: {0}, must be one of [auto, none, required, fn:<name>]")]
    UnknownToolChoice(String),
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
