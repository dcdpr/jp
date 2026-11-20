//! Loading backend trait for conversation data and indexes.

use std::fmt::Debug;

use camino::Utf8PathBuf;
use chrono::{DateTime, Utc};
use jp_config::PartialAppConfig;
use jp_conversation::{Conversation, ConversationEvent, ConversationId, ConversationStream};

use crate::{
    LoadError,
    validate::{DirContents, InvalidConversation, ValidationError},
};

/// Controls which storage partition to scan.
///
/// Live conversations are returned by default.
/// Set `archived` to `true` to scan the archive partition instead.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ConversationFilter {
    /// If true, scan the archive partition instead of the live one.
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
    /// By default, only live conversations are returned.
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
    ///
    /// A stored config that cannot be finalized on its own — a required field
    /// was among the ones recovery had to drop as unreadable — takes the
    /// missing values from `fallback`.
    /// Pass [`PartialAppConfig::empty()`] to hold the stored config to standing
    /// alone.
    fn load_conversation_stream(
        &self,
        id: &ConversationId,
        fallback: &PartialAppConfig,
    ) -> std::result::Result<ConversationStream, LoadError>;

    /// Load a single conversation's events in stream order without resolving
    /// its config.
    ///
    /// Selects the same storage root as [`Self::load_conversation_stream`], but
    /// does not read `base_config.json` or parse `config_delta` entries.
    /// A conversation whose stream load fails on either still returns its
    /// events.
    fn load_conversation_events(
        &self,
        id: &ConversationId,
    ) -> std::result::Result<Vec<ConversationEvent>, LoadError>;

    /// Return conversation IDs whose `expires_at` timestamp is in the past.
    ///
    /// Filesystem backends use a fast-path JSON reader that extracts only the
    /// `expires_at` field without deserializing the full `Conversation`.
    /// In-memory backends check the structs directly.
    fn load_expired_conversation_ids(&self, now: DateTime<Utc>) -> Vec<ConversationId>;

    /// Check the backing store and report the conversations that cannot be
    /// loaded.
    ///
    /// Filesystem backends check every conversation directory and remove
    /// leftovers of their own interrupted writes, but never move or delete a
    /// conversation.
    /// A conversation another process holds the lock on is left out of the
    /// report, because it may be partway through a write.
    /// In-memory backends hold only valid data and return an empty report.
    fn sanitize(&self) -> crate::error::Result<SanitizeReport>;

    /// What the store holds for conversation `id`, read without loading the
    /// conversation, so it describes one that fails to load or was never
    /// stored.
    ///
    /// `None` when the store holds nothing for `id`.
    /// In-memory backends hold only conversations that load, and return `None`.
    fn unlisted_conversation(&self, _id: &ConversationId) -> Option<UnlistedConversation> {
        None
    }
}

/// A conversation's stored files, as [`LoadBackend::unlisted_conversation`]
/// found them.
#[derive(Debug)]
pub struct UnlistedConversation {
    pub id: ConversationId,

    /// Where the store holds the conversation; for filesystem backends, its
    /// directories, workspace first.
    pub locations: Vec<Utf8PathBuf>,

    /// The conversation's files as startup validation checks them.
    ///
    /// The first failure when any location fails, otherwise
    /// [`DirContents::Stored`] when any location holds a stored conversation,
    /// [`DirContents::Empty`] when every location is empty, and
    /// [`DirContents::Unstored`] otherwise.
    pub state: Result<DirContents, ValidationError>,

    /// The title stored in the conversation's metadata, when the metadata loads
    /// and has one.
    pub title: Option<String>,
}

/// The conversations [`LoadBackend::sanitize`] found it cannot load.
#[derive(Debug, Default)]
pub struct SanitizeReport {
    /// Conversation directories that failed validation, sorted by path.
    pub invalid: Vec<InvalidConversation>,
}
