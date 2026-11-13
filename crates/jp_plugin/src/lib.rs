//! Protocol types for JP's command plugin system.
//!
//! Command plugins are standalone binaries (`jp-<name>`) that communicate with
//! JP over a JSON-lines protocol on stdin/stdout.
//! This crate defines the message types used by both sides.
//!
//! [`client`] implements the plugin-side handshake, message helpers, and binary
//! `main` entry point.
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
