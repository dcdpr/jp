//! Filesystem-backed storage backend.
//!
//! [`FsStorageBackend`] wraps [`Storage`] and implements all four backend
//! traits.
//! The `Storage` struct remains as an internal implementation detail; external
//! code interacts through the traits.

use std::collections::HashSet;

use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, Utc};
use jp_config::PartialAppConfig;
use jp_conversation::{Conversation, ConversationEvent, ConversationId, ConversationStream};
use rayon::iter::{IntoParallelIterator as _, ParallelIterator as _};
use relative_path::RelativePath;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tracing::debug;

use super::{
    ConversationFilter, ConversationIndexEntry, ConversationLockGuard, LoadBackend, LockBackend,
    PersistBackend, Projection, SanitizeReport, SessionBackend, UnlistedConversation,
};
use crate::{
    CONVERSATIONS_DIR, LoadError, Storage, conversation_dirs_for_id, dir_entries,
    error::Result,
    get_expiring_timestamp,
    load::load_json,
    load_conversation_id_from_entry,
    location::StorageLocation,
    lock::LockInfo,
    validate::{InvalidConversation, ValidationResult, validate_conversation_dir},
};

/// Filesystem-backed storage that delegates to `Storage`.
///
/// Implements all four backend traits.
/// Also exposes filesystem-specific methods (path accessors, etc.) that are not
/// part of the trait surface.
#[derive(Debug, Clone)]
pub struct FsStorageBackend {
    storage: Storage,
}

impl FsStorageBackend {
    /// Create a new filesystem backend at the given storage root.
    pub fn new(root: &Utf8Path) -> Result<Self> {
        Ok(Self {
            storage: Storage::new(root)?,
        })
    }

    /// Configure user-local storage for workspace `id` under `root`.
    ///
    /// The user-workspace directory is located by ID suffix, so every worktree
    /// and clone of a workspace shares one directory.
    /// A new one is named `<slug>-<id>` (or bare `<id>` when `slug` is absent
    /// or empty); `slug` only ever names a new directory and never renames an
    /// existing one.
    /// Runs a one-time migration on first setup that merges sibling
    /// user-workspace directories and imports the workspace's conversations so
    /// a durable user-local copy exists.
    pub fn with_user_storage(
        self,
        root: &Utf8Path,
        slug: Option<&str>,
        id: impl Into<String>,
    ) -> Result<Self> {
        Ok(Self {
            storage: self.storage.with_user_storage(root, slug, id)?,
        })
    }

    /// Attach the *existing* user-workspace directory for workspace `id`.
    ///
    /// Read-only counterpart of [`Self::with_user_storage`]: the directory is
    /// located by the same ID-suffix rule, but nothing is created, merged, or
    /// imported.
    /// User storage stays disabled when the workspace has none yet.
    #[must_use]
    pub fn with_existing_user_storage(self, root: &Utf8Path, slug: Option<&str>, id: &str) -> Self {
        Self {
            storage: self.storage.with_existing_user_storage(root, slug, id),
        }
    }

    /// Returns the path to the storage root directory.
    #[must_use]
    pub fn storage_path(&self) -> &Utf8Path {
        self.storage.path()
    }

    /// Returns the path to the user storage directory, if configured.
    #[must_use]
    pub fn user_storage_path(&self) -> Option<&Utf8Path> {
        self.storage.user_storage_path()
    }

    /// Return the absolute path to the given relative path, starting from the
    /// storage root.
    #[must_use]
    pub fn root_with_path(&self, path: &RelativePath) -> Utf8PathBuf {
        self.storage.root_with_path(path)
    }

    /// Return the absolute path to the given relative path, starting from the
    /// user storage directory.
    #[must_use]
    pub fn user_storage_with_path(&self, path: &RelativePath) -> Option<Utf8PathBuf> {
        self.storage.user_storage_with_path(path)
    }

    /// Return the absolute path to the given relative path, starting from the
    /// user or workspace storage directory.
    #[must_use]
    pub fn user_or_root_with_path(&self, path: &RelativePath) -> Utf8PathBuf {
        self.storage.user_or_root_with_path(path)
    }

    /// List orphaned lock file paths in user storage.
    ///
    /// Filesystem-specific: returns full paths for direct file removal.
    /// The trait method [`LockBackend::list_orphaned_locks`] returns
    /// conversation IDs instead.
    #[must_use]
    pub fn list_orphaned_lock_files(&self) -> Vec<Utf8PathBuf> {
        self.storage.list_orphaned_lock_files()
    }

    /// List session mapping file paths in user storage.
    ///
    /// Filesystem-specific: returns full paths.
    /// The trait method [`SessionBackend::list_session_keys`] returns just the
    /// session key strings.
    #[must_use]
    pub fn list_session_files(&self) -> Vec<Utf8PathBuf> {
        self.storage.list_session_files()
    }

    /// Read a JSON file from the storage.
    pub fn read_json<T: DeserializeOwned>(
        &self,
        path: &Utf8Path,
    ) -> std::result::Result<T, LoadError> {
        load_json(path)
    }

    /// Build the expected conversation directory path.
    ///
    /// Constructs the path where a conversation *would* be stored, regardless
    /// of whether the directory exists on disk.
    #[must_use]
    pub fn build_conversation_dir(
        &self,
        id: &ConversationId,
        title: Option<&str>,
        user: bool,
    ) -> Utf8PathBuf {
        self.storage.build_conversation_dir(id, title, user)
    }

    /// Find the directory path for a conversation by ID.
    ///
    /// Searches both workspace and user storage roots.
    /// Returns `None` if no matching directory exists.
    #[must_use]
    pub fn find_conversation_dir(&self, id: &ConversationId) -> Option<Utf8PathBuf> {
        self.storage.find_conversation_dir(id)
    }

    /// Find the conversation directory in the user-local store.
    ///
    /// Returns `None` when user-local storage is unconfigured, or when no
    /// directory there matches the ID.
    #[must_use]
    pub fn find_user_local_conversation_dir(&self, id: &ConversationId) -> Option<Utf8PathBuf> {
        self.storage.find_user_local_conversation_dir(id)
    }

    /// Path to a conversation's `events.json` file, if the directory exists.
    #[must_use]
    pub fn conversation_events_path(&self, id: &ConversationId) -> Option<Utf8PathBuf> {
        self.storage.conversation_events_path(id)
    }

    /// Path to a conversation's `metadata.json` file, if the directory exists.
    #[must_use]
    pub fn conversation_metadata_path(&self, id: &ConversationId) -> Option<Utf8PathBuf> {
        self.storage.conversation_metadata_path(id)
    }

    /// Path to a conversation's `base_config.json` file, if the directory
    /// exists.
    #[must_use]
    pub fn conversation_base_config_path(&self, id: &ConversationId) -> Option<Utf8PathBuf> {
        self.storage.conversation_base_config_path(id)
    }

    /// The live `conversations/` directory of each storage root, workspace
    /// first.
    ///
    /// The directories need not exist.
    #[must_use]
    pub fn conversations_dirs(&self) -> Vec<Utf8PathBuf> {
        [Some(self.storage.path()), self.storage.user_storage_path()]
            .into_iter()
            .flatten()
            .map(|root| root.join(CONVERSATIONS_DIR))
            .collect()
    }

    /// Every directory of conversation `id` among the live conversations of
    /// each storage root, workspace first, whether or not it loads.
    ///
    /// Lists only `<id>` and `<id>-<title>` directly inside a `conversations/`
    /// directory: never the archive, other hidden entries, or a directory whose
    /// name merely starts with the same digits.
    #[must_use]
    pub fn live_conversation_dirs(&self, id: &ConversationId) -> Vec<Utf8PathBuf> {
        let prefix = id.to_dirname(None);
        self.conversations_dirs()
            .iter()
            .flat_map(|dir| conversation_dirs_for_id(dir, &prefix))
            .collect()
    }

    /// Check every conversation directory in both roots without moving any.
    ///
    /// Sorts each directory into valid or invalid.
    /// See [`Storage::validate_conversations`] for the checks.
    #[must_use]
    pub fn validate_conversations(&self) -> ValidationResult {
        self.storage.validate_conversations()
    }

    /// Synchronize a projected conversation's user-local copy from the
    /// workspace copy after a managed editor command.
    ///
    /// A local-only conversation (no workspace copy) is left untouched.
    pub fn sync_projection(&self, id: &ConversationId) -> Result<()> {
        self.storage.sync_projection(id)
    }
}

impl PersistBackend for FsStorageBackend {
    fn write(
        &self,
        id: &ConversationId,
        metadata: &Conversation,
        events: &ConversationStream,
        projection: Projection,
    ) -> Result<()> {
        self.storage
            .persist_conversation(id, metadata, events, projection)
    }

    fn remove(&self, id: &ConversationId) -> Result<()> {
        self.storage.remove_conversation(id)
    }

    fn archive(&self, id: &ConversationId) -> Result<()> {
        self.storage.archive_conversation(id)
    }

    fn unarchive(&self, id: &ConversationId) -> Result<()> {
        self.storage.unarchive_conversation(id)
    }
}

impl LoadBackend for FsStorageBackend {
    fn load_conversation_index(&self, filter: ConversationFilter) -> Vec<ConversationIndexEntry> {
        self.storage.load_conversation_index(filter)
    }

    fn load_conversation_metadata(
        &self,
        id: &ConversationId,
    ) -> std::result::Result<Conversation, LoadError> {
        self.storage
            .load_conversation_metadata(id)
            .or_else(|_| self.storage.load_archived_conversation_metadata(id))
    }

    fn load_conversation_metadata_batch(
        &self,
        ids: &[ConversationId],
    ) -> Vec<(ConversationId, std::result::Result<Conversation, LoadError>)> {
        self.storage.load_conversation_metadata_batch(ids)
    }

    fn load_conversation_stream(
        &self,
        id: &ConversationId,
        fallback: &PartialAppConfig,
    ) -> std::result::Result<ConversationStream, LoadError> {
        self.storage.load_conversation_stream(id, fallback)
    }

    fn load_conversation_events(
        &self,
        id: &ConversationId,
    ) -> std::result::Result<Vec<ConversationEvent>, LoadError> {
        self.storage.load_conversation_events(id)
    }

    fn load_expired_conversation_ids(&self, now: DateTime<Utc>) -> Vec<ConversationId> {
        let storage = &self.storage;
        let roots: Vec<&Utf8Path> = [Some(storage.path()), storage.user_storage_path()]
            .into_iter()
            .flatten()
            .collect();

        let mut expired = vec![];
        for root in roots {
            let path = root.join(CONVERSATIONS_DIR);

            // Expiry deletes a conversation, which is only JP's to do for one
            // that loads.
            let valid: HashSet<_> = storage.valid_conversation_ids(&path).into_iter().collect();
            let ids: Vec<_> = dir_entries(&path)
                .collect::<Vec<_>>()
                .into_par_iter()
                .filter_map(|entry| {
                    let id = load_conversation_id_from_entry(&entry)?;
                    if !valid.contains(&id) {
                        return None;
                    }
                    let path = entry.into_path();
                    let expiring_ts = get_expiring_timestamp(&path)?;
                    (expiring_ts <= now).then_some(id)
                })
                .collect();
            expired.extend(ids);
        }
        // A projected conversation is found in both roots; collapse the
        // duplicates so callers remove it once.
        expired.sort();
        expired.dedup();
        expired
    }

    fn sanitize(&self) -> Result<SanitizeReport> {
        let validation = self.storage.validate_conversations();

        // A conversation another process holds may be between the writes of
        // its managed files, so its state says nothing yet.
        let unlocked = |id: &ConversationId| {
            let locked = self.storage.is_conversation_locked(&id.to_string());
            if locked {
                debug!(%id, "Ignoring locked conversation.");
            }
            !locked
        };

        let mut invalid: Vec<_> = validation
            .invalid
            .into_iter()
            .filter(|entry| unlocked(&entry.id))
            .collect();
        invalid.sort_by_key(InvalidConversation::path);

        Ok(SanitizeReport { invalid })
    }

    fn unlisted_conversation(&self, id: &ConversationId) -> Option<UnlistedConversation> {
        let dirs = self.live_conversation_dirs(id);
        if dirs.is_empty() {
            return None;
        }

        let state = dirs
            .iter()
            .try_for_each(|dir| validate_conversation_dir(dir));
        let locations = dirs
            .iter()
            .map(|dir| StorageLocation::of_path(dir))
            .collect();
        let title = self
            .storage
            .load_conversation_metadata(id)
            .ok()
            .and_then(|metadata| metadata.title);

        Some(UnlistedConversation {
            id: *id,
            locations,
            state,
            title,
        })
    }
}

impl LockBackend for FsStorageBackend {
    fn try_lock(
        &self,
        conversation_id: &str,
        session: Option<&str>,
    ) -> Result<Option<Box<dyn ConversationLockGuard>>> {
        match self
            .storage
            .try_lock_conversation(conversation_id, session)?
        {
            Some(guard) => Ok(Some(Box::new(guard))),
            None => Ok(None),
        }
    }

    fn lock_info(&self, conversation_id: &str) -> Option<LockInfo> {
        self.storage.read_conversation_lock_info(conversation_id)
    }

    fn list_orphaned_locks(&self) -> Vec<ConversationId> {
        self.storage
            .list_orphaned_lock_files()
            .into_iter()
            .filter_map(|path| {
                let stem = path.file_stem()?;
                stem.parse().ok()
            })
            .collect()
    }
}

impl SessionBackend for FsStorageBackend {
    fn load_session(&self, session_key: &str) -> Result<Option<Value>> {
        self.storage.load_session_data(session_key)
    }

    fn save_session(&self, session_key: &str, data: &Value) -> Result<()> {
        self.storage.save_session_data(session_key, data)
    }

    fn list_session_keys(&self) -> Vec<String> {
        self.storage
            .list_session_files()
            .into_iter()
            .filter_map(|path| path.file_stem().map(str::to_owned))
            .collect()
    }
}

#[cfg(debug_assertions)]
impl FsStorageBackend {
    /// Write a minimal valid conversation to storage.
    /// For test fixture setup.
    #[doc(hidden)]
    pub fn write_test_conversation(&self, id: &ConversationId, conversation: &Conversation) {
        self.storage.write_test_conversation(id, conversation);
    }

    /// Read the raw persisted events file content.
    /// For test assertions.
    #[doc(hidden)]
    #[must_use]
    pub fn read_test_events_raw(&self, id: &ConversationId) -> Option<String> {
        self.storage.read_test_events_raw(id)
    }

    /// Create a conversation directory whose `metadata.json` is not valid JSON.
    /// For test fixture setup.
    #[doc(hidden)]
    pub fn create_corrupt_test_conversation_dir(&self, dirname: &str) -> std::io::Result<()> {
        self.storage.create_corrupt_test_conversation_dir(dirname)
    }
}
