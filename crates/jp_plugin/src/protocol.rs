//! Protocol constants and helpers.

/// Current protocol version.
pub const PROTOCOL_VERSION: u32 = 1;

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
