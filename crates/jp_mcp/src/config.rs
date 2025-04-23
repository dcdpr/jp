use std::fmt;

use serde::{Deserialize, Serialize};

use crate::transport::Transport;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct McpServerId(String);

impl McpServerId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl fmt::Display for McpServerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Configuration for an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct McpServer {
    #[serde(skip)]
    pub id: McpServerId,
    pub transport: Transport,
}
