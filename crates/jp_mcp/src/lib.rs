mod client;
pub mod error;
pub mod id;

pub use client::Client;
pub use error::Error;
pub use rmcp::model::{CallToolResult, Content, RawContent, ResourceContents, Tool};
