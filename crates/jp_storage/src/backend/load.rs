//! Loading backend trait for conversation data and indexes.

use std::fmt::Debug;

use chrono::{DateTime, Utc};
use jp_conversation::{Conversation, ConversationId, ConversationStream};

use crate::{LoadError, validate::ValidationError};

/// Controls which storage partition to scan.
///
/// Active (non-archived) conversations are returned by default. Set `archived`
/// to `true` to scan the archive partition instead.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ConversationFilter {
    /// If true, scan the archive partition instead of the active one.
    pub archived: bool,
}

/// Reads conversation data and indexes from a backing store.
pub trait LoadBackend: Send + Sync + Debug {
    /// Scan conversation IDs from the backing store.
    ///
    /// The `filter` controls which partition to scan. By default, only active
    /// (non-archived) conversations are returned.
    fn load_conversation_ids(&self, filter: ConversationFilter) -> Vec<ConversationId>;

    /// Load a single conversation's metadata.
    fn load_conversation_metadata(
        &self,
        id: &ConversationId,
    ) -> std::result::Result<Conversation, LoadError>;

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
    /// Call this before [`Self::load_all_conversation_ids`] to guarantee the
    /// store is in a consistent state.
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
