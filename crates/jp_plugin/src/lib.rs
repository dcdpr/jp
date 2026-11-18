//! Protocol types for JP's command plugin system.
//!
//! Command plugins are standalone binaries (`jp-<name>`) that communicate with
//! JP over a JSON-lines protocol on stdin/stdout.
//! This crate defines the message types used by both sides.
//! [`stdio`] is the plugin's end of an exchange it reads and writes in turn.
//! `client`, behind the `client` feature, is the plugin's end of a connection
//! it shares between concurrent requests, and `log` sends the plugin's log
//! events to the host through it.
//! A plugin serving pages to the user's browser listens where [`server`] says,
//! and checks each request against [`origin`].
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
