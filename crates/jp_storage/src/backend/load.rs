//! Loading backend trait for conversation data and indexes.

use std::fmt::Debug;

use chrono::{DateTime, Utc};
use jp_conversation::{Conversation, ConversationId, ConversationStream};

use crate::{LoadError, validate::ValidationError};

/// Controls which storage partition to scan.
///
/// Active (non-archived) conversations are returned by default.
/// Set `archived` to `true` to scan the archive partition instead.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ConversationFilter {
    /// If true, scan the archive partition instead of the active one.
    pub archived: bool,
}

/// Which storage roots hold a conversation, as observed at load time.
///
/// Recorded per conversation in the workspace index and used to derive the
/// `local` indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoragePresence {
    /// Durable user-local copy only; not projected into this workspace.
    UserLocalOnly,
    /// Durable user-local copy plus a workspace projection.
    Projected,
    /// Present only in the workspace, not yet imported into user-local.
    WorkspaceOnly,
}

/// A conversation ID paired with where its data lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversationIndexEntry {
    /// The conversation's ID.
    pub id: ConversationId,

    /// Which storage roots hold the conversation.
    pub presence: StoragePresence,
}

/// Reads conversation data and indexes from a backing store.
pub trait LoadBackend: Send + Sync + Debug {
    /// Scan conversation index entries — ID plus [`StoragePresence`] — from
    /// the backing store.
    ///
    /// Filesystem backends merge both storage roots and deduplicate by
    /// conversation ID, so a projected conversation (present in both roots)
    /// appears exactly once.
    /// The `filter` controls which partition to scan.
    fn load_conversation_index(&self, filter: ConversationFilter) -> Vec<ConversationIndexEntry>;

    /// Scan conversation IDs from the backing store, deduplicated by ID.
    ///
    /// The `filter` controls which partition to scan.
    /// By default, only active (non-archived) conversations are returned.
    fn load_conversation_ids(&self, filter: ConversationFilter) -> Vec<ConversationId> {
        self.load_conversation_index(filter)
            .into_iter()
            .map(|entry| entry.id)
            .collect()
    }

    /// Load a single conversation's metadata.
    fn load_conversation_metadata(
        &self,
        id: &ConversationId,
    ) -> std::result::Result<Conversation, LoadError>;

    /// Load metadata for many conversations at once.
    ///
    /// Backends that resolve directories per id (filesystem) override this to
    /// scan once instead of once per conversation.
    /// The default loads each id individually.
    fn load_conversation_metadata_batch(
        &self,
        ids: &[ConversationId],
    ) -> Vec<(ConversationId, std::result::Result<Conversation, LoadError>)> {
        ids.iter()
            .map(|id| (*id, self.load_conversation_metadata(id)))
            .collect()
    }

    /// Load a single conversation's event stream.
    fn load_conversation_stream(
        &self,
        id: &ConversationId,
    ) -> std::result::Result<ConversationStream, LoadError>;

    /// Return conversation IDs whose `expires_at` timestamp is in the past.
    ///
    /// Filesystem backends use a fast-path JSON reader that extracts only the
    /// `expires_at` field without deserializing the full `Conversation`.
    /// In-memory backends check the structs directly.
    fn load_expired_conversation_ids(&self, now: DateTime<Utc>) -> Vec<ConversationId>;

    /// Validate and repair the backing store.
    ///
    /// For filesystem backends, this scans conversation directories, trashes
    /// corrupt entries to `.trash/`, and returns a report of what was repaired.
    /// For in-memory backends, data is always structurally valid, so this
    /// returns an empty report.
    ///
    /// Call this before [`Self::load_conversation_ids`] to guarantee the store
    /// is in a consistent state.
    fn sanitize(&self) -> crate::error::Result<SanitizeReport>;
}

/// Report of actions taken by [`LoadBackend::sanitize`].
#[derive(Debug, Default)]
pub struct SanitizeReport {
    /// Conversations that were moved to `.trash/` (or equivalent).
    pub trashed: Vec<TrashedConversation>,
}

impl SanitizeReport {
    /// Returns `true` if any repairs were made.
    #[must_use]
    pub fn has_repairs(&self) -> bool {
        !self.trashed.is_empty()
    }
}

/// A conversation that was trashed during sanitization.
#[derive(Debug)]
pub struct TrashedConversation {
    /// The original directory name (or equivalent identifier).
    pub dirname: String,

    /// The reason this conversation was trashed.
    pub error: ValidationError,
}
