//! Protocol types and helpers for JP's command plugin system.
//!
//! Command plugins are standalone binaries (`jp-<name>`) that communicate with
//! JP over a JSON-lines protocol on stdin/stdout.
//! This crate defines the message types used by both sides:
//!
//! - [`stdio`] provides synchronous request/response helpers for single-shot
//!   CLI plugins.
//! - `client` (behind the `client` feature) provides a multiplexed async client
//!   for long-running server plugins, and `log` forwards structured log events
//!   to the host over the same connection.
//! - [`server`] resolves bind addresses, ports, and allowed hostnames for HTTP
//!   server plugins, and [`origin`] validates request `Origin`,
//!   `Sec-Fetch-Site`, and `Host` headers.
//!
//! See: `docs/rfd/072-command-plugin-system.md`

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "client")]
pub mod log;
pub mod manifest;
pub mod message;
pub mod origin;
mod protocol;
pub mod registry;
pub mod server;
pub mod stdio;

pub use manifest::{Manifest, ManifestError};
pub use message::{HostToPlugin, PluginToHost};
pub use protocol::{Error, PROTOCOL_VERSION, ready};

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
