//! Protocol types for JP's command plugin system.
//!
//! Command plugins are standalone binaries (`jp-<name>`) that communicate with
//! JP over a JSON-lines protocol on stdin/stdout. This crate defines the
//! message types used by both sides.
//!
//! See: `docs/rfd/D17-command-plugin-system.md`

pub mod message;
mod protocol;
pub mod registry;

pub use message::{HostToPlugin, PluginToHost};
pub use protocol::{Error, PROTOCOL_VERSION};

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
