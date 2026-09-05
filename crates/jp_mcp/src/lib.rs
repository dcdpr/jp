//! MCP (Model Context Protocol) client integration for JP.

mod client;
pub mod error;
pub mod id;

pub use client::{Client, Startup, StartupSet, StderrLine};
pub use error::Error;
pub use rmcp::model::{CallToolResult, Content, RawContent, ResourceContents, Tool};
