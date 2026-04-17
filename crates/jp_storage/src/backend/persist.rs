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

    /// Move a conversation to the archive partition.
    ///
    /// The conversation directory is moved from the active partition into an
    /// archive area. Archived conversations are excluded from normal index
    /// scans and only visible through [`LoadBackend::load_conversation_ids`]
    /// with `ConversationFilter { archived: true }`.
    ///
    /// [`LoadBackend::load_conversation_ids`]: super::LoadBackend::load_conversation_ids
    fn archive(&self, id: &ConversationId) -> Result<()>;

    /// Restore a conversation from the archive partition to the active one.
    fn unarchive(&self, id: &ConversationId) -> Result<()>;
}
