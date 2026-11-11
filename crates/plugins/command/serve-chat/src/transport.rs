//! Transport-neutral chat types shared between the bridge and chat backends.
//!
//! A transport sends [`Inbound`] messages over an `mpsc` channel and accepts
//! outbound replies and typing indicators through [`Outbox`].

use std::fmt;

/// Chat destination where a message originated and where replies are sent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Chat {
    /// Direct 1:1 chat, identified by the sender's transport account ID.
    Direct(String),

    /// Group chat, identified by the transport group ID.
    Group(String),
}

impl Chat {
    /// Stable key (`direct:<id>` or `group:<id>`) used in `state.json` and
    /// logs.
    pub(crate) fn key(&self) -> String {
        match self {
            Self::Direct(id) => format!("direct:{id}"),
            Self::Group(id) => format!("group:{id}"),
        }
    }
}

impl fmt::Display for Chat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key())
    }
}

/// Inbound message received from a chat transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Inbound {
    pub chat: Chat,

    /// Transport identifiers for the sender, ordered most-stable first (account
    /// UUID before phone number, which a sender may hide).
    pub sender: Vec<String>,

    /// Display name from the sender's profile, if reported by the transport.
    pub name: Option<String>,

    /// Whether the message mentions the bridge's own account or replies to one
    /// of its messages.
    pub addressed: bool,

    /// Message text; empty if the message contained only attachments.
    pub text: String,

    /// Number of attachments on the message.
    pub attachments: usize,
}

/// Outbound interface for sending messages and typing indicators to a chat app.
///
/// Methods enqueue the request and return immediately; delivery errors are
/// logged by the transport.
pub(crate) trait Outbox: Send + Sync {
    /// Convert `markdown` to the transport's text format and send it to `chat`.
    fn send(&self, chat: &Chat, markdown: &str);

    /// Start or stop the typing indicator in `chat`.
    fn typing(&self, chat: &Chat, on: bool);
}
