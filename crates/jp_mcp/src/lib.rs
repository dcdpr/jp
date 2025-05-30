mod client;
pub mod config;
pub mod error;
pub mod server;
pub mod tool;
pub mod transport;

pub use client::Client;
pub use error::Error;
pub use rmcp::model::{Content, RawContent, ResourceContents, Tool};

#[derive(Debug, Clone, serde::Serialize)]
pub struct CallToolResult {
    pub id: String,
    pub content: Vec<Content>,
    pub error: bool,
}
