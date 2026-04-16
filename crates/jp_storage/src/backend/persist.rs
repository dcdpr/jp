//! Persistence backend trait for conversation data.

use std::fmt::Debug;

use jp_conversation::{Conversation, ConversationId, ConversationStream};

use crate::error::Result;

/// Writes and removes conversation data.
///
/// Implementations persist a single conversation's metadata and events to some
/// backend (filesystem, in-memory, null/no-op, etc.).
pub trait PersistBackend: Send + Sync + Debug {
    /// Persist a conversation's metadata and events.
    fn write(
        &self,
        id: &ConversationId,
        metadata: &Conversation,
        events: &ConversationStream,
    ) -> Result<()>;

    /// Remove a conversation's persisted data entirely.
    fn remove(&self, id: &ConversationId) -> Result<()>;
}
