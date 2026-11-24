//! Filesystem-backed storage backend.
//!
//! [`FsStorageBackend`] wraps [`Storage`] and implements all five backend
//! traits.
//! The `Storage` struct remains as an internal implementation detail; external
//! code interacts through the traits.

use std::{collections::HashSet, fs};

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
    ConversationFilter, ConversationIndexEntry, ConversationLockGuard, DiagnosedConversation,
    Diagnosis, LoadBackend, LockBackend, PersistBackend, Projection, RecoveryBackend, Removal,
    Repair, RepairOutcome, SessionBackend, StoredConversation, StrayEntry, StrayKind,
};
use crate::{
    ARCHIVE_DIR, CONVERSATIONS_DIR, IMPORT_STAGING_PREFIX, LoadError, Storage,
    conversation_dirs_for_id, dir_entries,
    error::Result,
    get_expiring_timestamp,
    load::load_json,
    load_conversation_id_from_entry,
    location::StorageLocation,
    lock::LockInfo,
    rebuild::rebuild_metadata,
    resource_lock::ResourceGuard,
    validate::{
        InvalidConversation, ValidationResult, missing_only_metadata, validate_conversation_dir,
    },
};

/// Filesystem-backed storage that delegates to `Storage`.
///
/// Implements all five backend traits.
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

    /// Block until this process holds the store-wide lock named `resource`; see
    /// [`Storage::lock_store`].
    ///
    /// # Errors
    ///
    /// Fails when the lock file cannot be created or locked.
    pub fn lock_store(&self, resource: &str) -> Result<Box<dyn ResourceGuard>> {
        self.storage.lock_store(resource)
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

    /// Return the live `conversations/` directory for each configured storage
    /// root, ordered with workspace storage before user-local storage.
    ///
    /// The returned paths may not exist on disk.
    #[must_use]
    pub fn conversations_dirs(&self) -> Vec<Utf8PathBuf> {
        [Some(self.storage.path()), self.storage.user_storage_path()]
            .into_iter()
            .flatten()
            .map(|root| root.join(CONVERSATIONS_DIR))
            .collect()
    }

    /// Return every directory matching conversation `id` in the live
    /// `conversations/` roots, workspace first, regardless of whether it passes
    /// validation.
    ///
    /// Matches only `<id>` and `<id>-<title>` directly under `conversations/`,
    /// excluding `.archive/`, hidden directories, and directories that merely
    /// share a numeric prefix.
    #[must_use]
    pub fn live_conversation_dirs(&self, id: &ConversationId) -> Vec<Utf8PathBuf> {
        let prefix = id.to_dirname(None);
        self.conversations_dirs()
            .iter()
            .flat_map(|dir| conversation_dirs_for_id(dir, &prefix))
            .collect()
    }

    /// Validate all conversation directories across both storage roots without
    /// modifying them; see [`Storage::validate_conversations`].
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

    fn conversation_locations(&self, id: &ConversationId) -> Vec<StorageLocation> {
        let (dirs, _) = self.stored_dirs(id);
        dirs.iter()
            .map(|dir| StorageLocation::of_path(dir))
            .collect()
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

            // Only expire conversations that pass validation; invalid
            // conversations are left untouched for the user to inspect.
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
}

impl RecoveryBackend for FsStorageBackend {
    fn scan(&self) -> Result<Vec<InvalidConversation>> {
        let validation = self.storage.validate_conversations();

        // Skip locked conversations, which may be mid-write in another process.
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

        Ok(invalid)
    }

    fn scan_archive(&self) -> Result<Vec<InvalidConversation>> {
        let mut invalid: Vec<_> = self
            .storage
            .validate_archive()?
            .into_iter()
            .filter(|entry| !self.storage.is_conversation_locked(&entry.id.to_string()))
            .collect();
        invalid.sort_by_key(InvalidConversation::path);

        Ok(invalid)
    }

    fn inspect(&self, id: &ConversationId) -> Option<StoredConversation> {
        self.stored(id).map(|(stored, _)| stored)
    }

    fn remove(
        &self,
        id: &ConversationId,
        decide: &mut dyn FnMut(&StoredConversation) -> bool,
    ) -> Result<Removal> {
        // Hold the conversation lock until removal completes so we never delete
        // an in-flight write from another process.
        let Some(_guard) = self.storage.try_lock_conversation(&id.to_string(), None)? else {
            return Ok(Removal::Locked);
        };

        let Some((stored, dirs)) = self.stored(id) else {
            return Ok(Removal::NotFound);
        };
        if !decide(&stored) {
            return Ok(Removal::Declined);
        }

        for dir in dirs {
            fs::remove_dir_all(dir)?;
        }
        Ok(Removal::Removed)
    }

    fn diagnose(&self) -> Result<Diagnosis> {
        let found = self.storage.diagnose_conversations()?;

        let invalid = found
            .invalid
            .into_iter()
            .filter(|entry| !self.storage.is_conversation_locked(&entry.id.to_string()))
            .map(|conversation| {
                // A lookup by ID prefers the live copy, so when both a live and
                // an archived copy exist, only the live one can be repaired or
                // removed by ID.
                let (dirs, archived) = self.stored_dirs(&conversation.id);
                let shadowed = archived != conversation.archived;
                let repair = if shadowed {
                    None
                } else {
                    applicable_repair(&dirs)
                };
                DiagnosedConversation {
                    conversation,
                    repair,
                    shadowed,
                }
            })
            .collect();

        let mut strays: Vec<_> = found
            .unrecognized
            .iter()
            .map(|path| stray_entry(path, StrayKind::NotAConversation))
            .chain(found.leftovers.iter().map(|path| {
                let staging = path
                    .file_name()
                    .is_some_and(|name| name.starts_with(IMPORT_STAGING_PREFIX));
                let kind = if staging {
                    StrayKind::ImportStaging
                } else {
                    StrayKind::TempFile
                };
                stray_entry(path, kind)
            }))
            .collect();
        strays.sort_by(|a, b| a.location.as_str().cmp(b.location.as_str()));

        Ok(Diagnosis { invalid, strays })
    }

    fn repair(
        &self,
        id: &ConversationId,
        repair: Repair,
        decide: &mut dyn FnMut(&StoredConversation) -> bool,
    ) -> Result<RepairOutcome> {
        let Some(_guard) = self.storage.try_lock_conversation(&id.to_string(), None)? else {
            return Ok(RepairOutcome::Locked);
        };

        let Some((stored, dirs)) = self.stored(id) else {
            return Ok(RepairOutcome::NotFound);
        };
        if stored.repair != Some(repair) {
            return Ok(RepairOutcome::NotApplicable);
        }
        if !decide(&stored) {
            return Ok(RepairOutcome::Declined);
        }

        match repair {
            Repair::RebuildMetadata => {
                for dir in &dirs {
                    rebuild_metadata(dir, id, stored.archived)?;
                }
            }
        }

        Ok(RepairOutcome::Repaired)
    }
}

impl FsStorageBackend {
    /// Return every directory matching conversation `id` in each storage root's
    /// archive, workspace first.
    fn archived_conversation_dirs(&self, id: &ConversationId) -> Vec<Utf8PathBuf> {
        let prefix = id.to_dirname(None);
        self.conversations_dirs()
            .iter()
            .flat_map(|dir| conversation_dirs_for_id(&dir.join(ARCHIVE_DIR), &prefix))
            .collect()
    }

    /// Return the directories holding conversation `id` and whether they are in
    /// the archive.
    ///
    /// Prefers live directories; checks the archive only when no live directory
    /// exists for `id`.
    fn stored_dirs(&self, id: &ConversationId) -> (Vec<Utf8PathBuf>, bool) {
        let live = self.live_conversation_dirs(id);
        if !live.is_empty() {
            return (live, false);
        }
        (self.archived_conversation_dirs(id), true)
    }

    /// Inspect the on-disk state and directories for conversation `id`.
    fn stored(&self, id: &ConversationId) -> Option<(StoredConversation, Vec<Utf8PathBuf>)> {
        let (dirs, archived) = self.stored_dirs(id);
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
        let metadata = if archived {
            self.storage.load_archived_conversation_metadata(id)
        } else {
            self.storage.load_conversation_metadata(id)
        };

        let stored = StoredConversation {
            id: *id,
            locations,
            state,
            title: metadata.ok().and_then(|metadata| metadata.title),
            repair: applicable_repair(&dirs),
            archived,
        };
        Some((stored, dirs))
    }
}

/// Return the repair that can fix `dirs`, if every directory in the slice
/// requires the same repair.
///
/// If one copy is valid and another is broken, the copies disagree and no
/// automatic repair applies.
fn applicable_repair(dirs: &[Utf8PathBuf]) -> Option<Repair> {
    let rebuildable = !dirs.is_empty() && dirs.iter().all(|dir| missing_only_metadata(dir));
    rebuildable.then_some(Repair::RebuildMetadata)
}

/// Build a [`StrayEntry`] for `path`, named after its entry in `conversations/`
/// (or the parent conversation directory for a `.tmp` file).
fn stray_entry(path: &Utf8Path, kind: StrayKind) -> StrayEntry {
    let named = match kind {
        StrayKind::TempFile => path.parent(),
        StrayKind::NotAConversation | StrayKind::ImportStaging => Some(path),
    };
    let name = named
        .and_then(Utf8Path::file_name)
        .unwrap_or(path.as_str())
        .to_owned();

    StrayEntry {
        name,
        kind,
        location: StorageLocation::of_path(path),
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
