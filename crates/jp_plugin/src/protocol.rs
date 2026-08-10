//! Protocol constants and helpers.

use crate::message::{ExitMessage, ReadyMessage};

/// Current protocol version.
///
/// Bumped whenever the host gains a message or a message variant a plugin might
/// send.
///
/// | Version | Adds                                                            |
/// | ------- | --------------------------------------------------------------- |
/// | 1       | The initial protocol.                                           |
/// | 2       | `compose` / `composed`, for host-run prompts.                   |
/// | 3       | `archive_conversation` and `set_title`, answered with `done`.   |
/// | 4       | `read_draft` / `write_draft`, for a conversation's query draft. |
/// | 5       | `list_configs`, naming the configurations a query can select.   |
/// | 6       | `query`, with `created` and `query_complete` in reply.          |
pub const PROTOCOL_VERSION: u32 = 6;

/// Answer a host's `init`, refusing it when it is too old to serve this plugin.
///
/// `required` is the lowest protocol version the plugin can work with; `host`
/// is the version from [`crate::message::InitMessage`].
/// The `Err` case is the [`ExitMessage`] to send instead of going any further:
/// a plugin that carried on would send messages the host cannot read, and then
/// block on replies that never come.
pub fn ready(required: u32, host: u32) -> Result<ReadyMessage, ExitMessage> {
    if host < required {
        return Err(ExitMessage {
            code: 1,
            reason: Some(format!(
                "this plugin needs `jp` protocol {required}, and this `jp` speaks {host}. \
                 Reinstall the two together."
            )),
        });
    }

    Ok(ReadyMessage { protocol: required })
}

/// Errors that can occur during plugin protocol communication.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// JSON serialization or deserialization failed.
    #[error("protocol JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// An I/O error occurred reading from or writing to the plugin.
    #[error("protocol I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The plugin sent an unrecognized message type.
    #[error("unknown message type: {0}")]
    UnknownMessage(String),

    /// The plugin process exited without sending an `exit` message.
    #[error("plugin exited unexpectedly (status: {0})")]
    UnexpectedExit(String),

    /// The plugin binary was not found.
    #[error("plugin binary not found: {0}")]
    NotFound(String),

    /// The plugin sent an error response.
    #[error("plugin error: {0}")]
    PluginError(String),
}
