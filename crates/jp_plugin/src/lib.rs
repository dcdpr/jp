//! Protocol types for JP's command plugin system.
//!
//! Command plugins are standalone binaries (`jp-<name>`) that communicate with
//! JP over a JSON-lines protocol on stdin/stdout.
//! This crate defines the message types used by both sides.
//!
//! The `client` module is the plugin's side of the protocol: the handshake,
//! sending and awaiting messages, and the body of a plugin binary's `main`.
//!
//! See: `docs/rfd/072-command-plugin-system.md`

pub mod client;
pub mod manifest;
pub mod message;
mod protocol;
pub mod registry;

pub use manifest::{Manifest, ManifestError};
pub use message::{HostToPlugin, PluginToHost};
pub use protocol::{Error, PROTOCOL_VERSION, ready};

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
