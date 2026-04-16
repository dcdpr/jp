//! Filesystem-backed storage backend.
//!
//! [`FsStorageBackend`] wraps [`Storage`] and implements all four backend
//! traits. The `Storage` struct remains as an internal implementation detail;
//! external code interacts through the traits.

use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, Utc};
use jp_conversation::{Conversation, ConversationId, ConversationStream};
use rayon::iter::{IntoParallelIterator as _, ParallelIterator as _};
use relative_path::RelativePath;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tracing::{debug, error};

use super::{
    ConversationLockGuard, LoadBackend, LockBackend, PersistBackend, SanitizeReport,
    SessionBackend, TrashedConversation,
};
use crate::{
    CONVERSATIONS_DIR, LoadError, Storage, dir_entries,
    error::Result,
    get_expiring_timestamp,
    load::load_json,
    load_conversation_id_from_entry,
    lock::{ConversationFileLock, LockInfo},
    validate::trash_invalid_conversation,
};

/// Filesystem-backed storage that delegates to `Storage`.
///
/// Implements all four backend traits. Also exposes filesystem-specific methods
/// (path accessors, etc.) that are not part of the trait surface.
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

    /// Configure user-local storage.
    pub fn with_user_storage(
        self,
        root: &Utf8Path,
        name: impl AsRef<str>,
        id: impl Into<String>,
    ) -> Result<Self> {
        Ok(Self {
            storage: self.storage.with_user_storage(root, name, id)?,
        })
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
    /// Filesystem-specific: returns full paths for direct file removal. The
    /// trait method [`LockBackend::list_orphaned_locks`] returns conversation
    /// IDs instead.
    #[must_use]
    pub fn list_orphaned_lock_files(&self) -> Vec<Utf8PathBuf> {
        self.storage.list_orphaned_lock_files()
    }

    /// List session mapping file paths in user storage.
    ///
    /// Filesystem-specific: returns full paths. The trait method
    /// [`SessionBackend::list_session_keys`] returns just the session key
    /// strings.
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
    /// Searches both workspace and user storage roots. Returns `None` if no
    /// matching directory exists.
    #[must_use]
    pub fn find_conversation_dir(&self, id: &ConversationId) -> Option<Utf8PathBuf> {
        self.storage.find_conversation_dir(id)
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

    /// Path to a conversation's `base_config.json` file, if the directory exists.
    #[must_use]
    pub fn conversation_base_config_path(&self, id: &ConversationId) -> Option<Utf8PathBuf> {
        self.storage.conversation_base_config_path(id)
    }
}

impl PersistBackend for FsStorageBackend {
    fn write(
        &self,
        id: &ConversationId,
        metadata: &Conversation,
        events: &ConversationStream,
    ) -> Result<()> {
        self.storage.persist_conversation(id, metadata, events)
    }

    fn remove(&self, id: &ConversationId) -> Result<()> {
        self.storage.remove_conversation(id)
    }
}

impl LoadBackend for FsStorageBackend {
    fn load_all_conversation_ids(&self) -> Vec<ConversationId> {
        self.storage.load_all_conversation_ids()
    }

    fn load_conversation_metadata(
        &self,
        id: &ConversationId,
    ) -> std::result::Result<Conversation, LoadError> {
        self.storage.load_conversation_metadata(id)
    }

    fn load_conversation_stream(
        &self,
        id: &ConversationId,
    ) -> std::result::Result<ConversationStream, LoadError> {
        self.storage.load_conversation_stream(id)
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
            let ids: Vec<_> = dir_entries(&path)
                .collect::<Vec<_>>()
                .into_par_iter()
                .filter_map(|entry| {
                    let id = load_conversation_id_from_entry(&entry)?;
                    let path = entry.into_path();
                    let expiring_ts = get_expiring_timestamp(&path)?;
                    (expiring_ts <= now).then_some(id)
                })
                .collect();
            expired.extend(ids);
        }
        expired
    }

    fn sanitize(&self) -> Result<SanitizeReport> {
        let validation = self.storage.validate_conversations();
        let mut report = SanitizeReport::default();

        for entry in validation.invalid {
            // Skip conversations that are actively locked by another process.
            // This prevents a race where process A creates a conversation
            // directory (e.g. for QUERY_MESSAGE.md while the editor is open)
            // but hasn't persisted the managed files yet, and process B's
            // validation trashes it because metadata.json is missing.
            if let Ok(id) = ConversationId::try_from_dirname(&entry.dirname)
                && self.storage.is_conversation_locked(&id.to_string())
            {
                debug!(
                    dirname = entry.dirname,
                    error = %entry.error,
                    "Skipping locked conversation during sanitization."
                );
                continue;
            }

            if let Err(e) = trash_invalid_conversation(&entry) {
                error!(
                    dirname = entry.dirname,
                    error = %entry.error,
                    trash_error = %e,
                    "Failed to trash conversation, skipping"
                );
                continue;
            }

            report.trashed.push(TrashedConversation {
                dirname: entry.dirname.clone(),
                error: entry.error,
            });
        }

        Ok(report)
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
            Some(lock) => Ok(Some(Box::new(lock))),
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

impl ConversationLockGuard for ConversationFileLock {}

#[cfg(debug_assertions)]
impl FsStorageBackend {
    /// Write a minimal valid conversation to storage. For test fixture setup.
    #[doc(hidden)]
    pub fn write_test_conversation(&self, id: &ConversationId, conversation: &Conversation) {
        self.storage.write_test_conversation(id, conversation);
    }

    /// Read the raw persisted events file content. For test assertions.
    #[doc(hidden)]
    #[must_use]
    pub fn read_test_events_raw(&self, id: &ConversationId) -> Option<String> {
        self.storage.read_test_events_raw(id)
    }

    /// Create an empty conversation directory that will fail validation.
    #[doc(hidden)]
    pub fn create_test_conversation_dir(&self, dirname: &str) {
        self.storage.create_test_conversation_dir(dirname);
    }
}
