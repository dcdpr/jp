//! MCP (Model Context Protocol) integration for JP.
//!
//! Two halves, separately selectable:
//!
//! - `client` connects to the MCP servers named in `providers.mcp`.
//! - `server` runs JP's tools, whatever their source, and needs the client to
//!   reach the MCP-backed ones.

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "client")]
pub mod error;
pub mod id;
#[cfg(feature = "server")]
pub mod server;

#[cfg(feature = "client")]
pub use client::{Client, Startup, StartupSet, StderrLine};
#[cfg(feature = "client")]
pub use error::Error;
pub use rmcp::model::{CallToolResult, Content, RawContent, ResourceContents, Tool};
