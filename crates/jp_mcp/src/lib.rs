mod client;
pub mod config;
pub mod error;
pub mod server;
pub mod tool;
pub mod transport;

pub use client::Client;
pub use error::Error;
pub use rmcp::model::{CallToolResult, Content, RawContent, ResourceContents, Tool};
