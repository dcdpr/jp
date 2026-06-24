//! Persistence backend trait for conversation data.

use std::fmt::Debug;

use jp_conversation::{Conversation, ConversationId, ConversationStream};

use super::StoragePresence;
use crate::error::Result;

/// Where a conversation's data should be written.
///
/// Carried by a conversation lock as the write intent, independent of what is
/// currently on disk.
/// Resolved from a conversation's [`StoragePresence`] at lock acquisition, or
/// from the creation flags for a new conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Projection {
    /// Durable user-local copy only.
    /// Selected by `--local`.
    LocalOnly,
    /// Durable user-local copy plus a workspace projection.
    Projected,
}

impl From<StoragePresence> for Projection {
    fn from(presence: StoragePresence) -> Self {
        match presence {
            StoragePresence::UserLocalOnly => Self::LocalOnly,
            // A workspace-only (external) conversation is projected on its
            // first write, which also creates the durable user-local copy.
            StoragePresence::Projected | StoragePresence::WorkspaceOnly => Self::Projected,
        }
    }
}

impl From<Projection> for StoragePresence {
    fn from(projection: Projection) -> Self {
        match projection {
            Projection::LocalOnly => Self::UserLocalOnly,
            Projection::Projected => Self::Projected,
        }
    }
}

/// Writes and removes conversation data.
///
/// Implementations persist a single conversation's metadata and events to some
/// backend (filesystem, in-memory, null/no-op, etc.).
pub trait PersistBackend: Send + Sync + Debug {
    /// Persist a conversation's metadata and events.
    ///
    /// `projection` selects which storage roots receive the write: the durable
    /// user-local copy is always written, plus a workspace copy when
    /// [`Projection::Projected`].
    /// Backends with a single store ignore it.
    fn write(
        &self,
        id: &ConversationId,
        metadata: &Conversation,
        events: &ConversationStream,
        projection: Projection,
    ) -> Result<()>;

    /// Remove a conversation's persisted data entirely.
    fn remove(&self, id: &ConversationId) -> Result<()>;

    /// Move a conversation to the archive partition.
    ///
    /// The conversation directory is moved from the active partition into an
    /// archive area.
    /// Archived conversations are excluded from normal index scans and only
    /// visible through [`LoadBackend::load_conversation_ids`] with
    /// `ConversationFilter { archived: true }`.
    ///
    /// [`LoadBackend::load_conversation_ids`]: super::LoadBackend::load_conversation_ids
    fn archive(&self, id: &ConversationId) -> Result<()>;

    /// Restore a conversation from the archive partition to the active one.
    fn unarchive(&self, id: &ConversationId) -> Result<()>;
}
