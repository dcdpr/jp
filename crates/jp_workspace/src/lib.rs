//! JP Workspace: A crate for managing LLM-assisted code conversations
//!
//! This crate provides data models and storage operations for the JP workspace,
//! a CLI tool for managing LLM-assisted code conversations with fine-grained
//! control over context and behavior.

mod conversation_lock;
mod error;
mod handle;
mod id;
mod sanitize;
pub mod session;
pub(crate) mod session_mapping;
mod state;

use std::sync::{Arc, OnceLock};

use camino::{FromPathBufError, Utf8Path, Utf8PathBuf};
pub use conversation_lock::{ConversationLock, ConversationMut, LockResult};
pub use error::Error;
use error::Result;
pub use handle::ConversationHandle;
pub use id::Id;
use jp_config::AppConfig;
use jp_conversation::{Conversation, ConversationId, ConversationStream};
use jp_storage::{
    backend::{
        ConversationFilter, InMemoryStorageBackend, LoadBackend, LockBackend, NullPersistBackend,
        PersistBackend, SessionBackend,
    },
    lock::LockInfo,
};
use parking_lot::{ArcRwLockReadGuard, RawRwLock, RwLock, RwLockReadGuard};
use rayon::prelude::*;
pub use sanitize::{SanitizeReport, TrashedConversation};
use state::State;
use tracing::{debug, trace, warn};

use crate::session::Session;

const APPLICATION: &str = "jp";

#[derive(Debug)]
pub struct Workspace {
    /// The root directory of the workspace.
    root: Utf8PathBuf,

    /// The globally unique ID of the workspace.
    id: id::Id,

    /// Backend for writing/removing conversation data.
    persist: Arc<dyn PersistBackend>,

    /// Backend for reading conversation data and indexes.
    loader: Arc<dyn LoadBackend>,

    /// Backend for conversation-level locking.
    locker: Arc<dyn LockBackend>,

    /// Backend for session-to-conversation mapping storage.
    sessions: Arc<dyn SessionBackend>,

    /// The in-memory state of the workspace.
    state: State,
}

impl Workspace {
    /// Find the [`Workspace`] root by walking up the directory tree.
    #[must_use]
    pub fn find_root(mut current_dir: Utf8PathBuf, storage_dir: &str) -> Option<Utf8PathBuf> {
        if storage_dir.is_empty() {
            return None;
        }

        loop {
            let config_path = current_dir.join(storage_dir);
            if config_path.is_dir() {
                return Some(current_dir);
            }

            if !current_dir.pop() {
                return None;
            }
        }
    }

    /// Creates a new workspace with the given root directory.
    ///
    /// The workspace starts with in-memory backends (no filesystem
    /// persistence). Call [`with_backend`] to wire in a storage backend.
    ///
    /// [`with_backend`]: Self::with_backend
    pub fn new(root: impl Into<Utf8PathBuf>) -> Self {
        Self::new_with_id(root, id::Id::new())
    }

    /// Creates a new workspace with the given root directory and ID.
    ///
    /// All four backend slots are wired to a single shared
    /// [`InMemoryStorageBackend`], so data written through one trait is visible
    /// through the others.
    pub fn new_with_id(root: impl Into<Utf8PathBuf>, id: id::Id) -> Self {
        let root = root.into();
        trace!(root = %root, id = %id, "Initializing Workspace.");

        let backend = Arc::new(InMemoryStorageBackend::new());
        Self {
            root,
            id,
            persist: backend.clone(),
            loader: backend.clone(),
            locker: backend.clone(),
            sessions: backend,
            state: State::default(),
        }
    }

    /// Get the root path of the workspace.
    #[must_use]
    pub fn root(&self) -> &Utf8Path {
        &self.root
    }

    /// Set the persist backend.
    #[must_use]
    pub fn with_persist(mut self, persist: Arc<dyn PersistBackend>) -> Self {
        self.persist = persist;
        self
    }

    /// Set the load backend.
    #[must_use]
    pub fn with_loader(mut self, loader: Arc<dyn LoadBackend>) -> Self {
        self.loader = loader;
        self
    }

    /// Set the lock backend.
    #[must_use]
    pub fn with_locker(mut self, locker: Arc<dyn LockBackend>) -> Self {
        self.locker = locker;
        self
    }

    /// Set the session backend.
    #[must_use]
    pub fn with_sessions(mut self, sessions: Arc<dyn SessionBackend>) -> Self {
        self.sessions = sessions;
        self
    }

    /// Set all four backends from a single implementation.
    ///
    /// Convenience for types that implement all four backend traits.
    #[must_use]
    pub fn with_backend<T>(self, backend: Arc<T>) -> Self
    where
        T: PersistBackend + LoadBackend + LockBackend + SessionBackend + 'static,
    {
        self.with_persist(backend.clone())
            .with_loader(backend.clone())
            .with_locker(backend.clone())
            .with_sessions(backend)
    }

    /// Disable persistence for the workspace.
    ///
    /// Swaps the persist backend to [`NullPersistBackend`], which silently
    /// discards all writes. Already-created `ConversationMut` instances hold
    /// their own `Arc` clone of the original backend and continue to persist.
    pub fn disable_persistence(&mut self) {
        self.persist = Arc::new(NullPersistBackend);
    }

    /// Scan conversation IDs from the backing store and populate the workspace
    /// index.
    ///
    /// All entries are lazily initialized — metadata and events are loaded on
    /// first access via [`metadata`], [`events`], etc.
    ///
    /// Call [`sanitize`] before this method to ensure the backing store is in a
    /// consistent state.
    ///
    /// [`metadata`]: Self::metadata
    /// [`events`]: Self::events
    /// [`sanitize`]: Self::sanitize
    pub fn load_conversation_index(&mut self) {
        trace!("Loading conversation index.");
        let conversation_ids = self
            .loader
            .load_conversation_ids(ConversationFilter::default());

        debug!(count = conversation_ids.len(), "Loaded conversation index.");

        let conversations = conversation_ids
            .iter()
            .map(|id| (*id, OnceLock::new()))
            .collect();

        let events = conversation_ids
            .into_iter()
            .map(|id| (id, OnceLock::new()))
            .collect();

        self.state = State {
            conversations,
            events,
        };
    }

    /// Eagerly load a single conversation's metadata and events from the
    /// backing store.
    pub fn eager_load_conversation(&mut self, h: &ConversationHandle) -> Result<()> {
        let id = &h.id();

        if let Some(cell) = self.state.conversations.get(id)
            && cell.get().is_none()
        {
            let metadata = self.loader.load_conversation_metadata(id)?;
            let _err = cell.set(Arc::new(RwLock::new(metadata)));
        }

        if let Some(cell) = self.state.events.get(id)
            && cell.get().is_none()
        {
            let stream = self.loader.load_conversation_stream(id)?;
            let _err = cell.set(Arc::new(RwLock::new(stream)));
        }

        Ok(())
    }

    /// Remove expired ephemeral conversations.
    ///
    /// Scans the backing store for conversations whose `expires_at` timestamp
    /// is in the past, then removes them through the persist backend. If
    /// persistence is disabled (`NullPersistBackend`), the removes are no-ops.
    pub fn remove_ephemeral_conversations(&mut self, skip: &[ConversationId]) {
        let expired = self
            .loader
            .load_expired_conversation_ids(chrono::Utc::now());

        for id in expired {
            if skip.contains(&id) {
                continue;
            }
            if let Err(e) = self.persist.remove(&id) {
                warn!(%id, %e, "Failed to remove ephemeral conversation.");
            }
        }
    }

    /// Returns an iterator over all conversations.
    ///
    /// Uninitialized metadata is loaded from the backing store in parallel (via
    /// rayon) on first access. Already-loaded conversations are returned as-is.
    ///
    /// Each item yields a read guard that auto-derefs to `&Conversation`. Do
    /// **not** hold these guards across `.await` points.
    pub fn conversations(
        &self,
    ) -> impl Iterator<Item = (&ConversationId, ArcRwLockReadGuard<RawRwLock, Conversation>)> {
        self.ensure_all_metadata_loaded();

        self.state
            .conversations
            .iter()
            .filter_map(|(id, cell)| cell.get().map(|arc| (id, arc.read_arc())))
    }

    /// Create a new conversation in memory.
    ///
    /// Returns the conversation ID. No data is written to the backing store and
    /// no cross-process lock is acquired. Persistence happens when a
    /// [`ConversationMut`] holding this conversation is flushed or dropped.
    ///
    /// For code that needs cross-process exclusion from the start, use
    /// [`create_and_lock_conversation`] instead.
    ///
    /// [`create_and_lock_conversation`]: Self::create_and_lock_conversation
    pub fn create_conversation(
        &mut self,
        conversation: Conversation,
        config: Arc<AppConfig>,
    ) -> ConversationId {
        self.create_conversation_with_id(ConversationId::default(), conversation, config)
    }

    /// Create a new conversation in memory with a specific ID.
    ///
    /// See [`create_conversation`] for details.
    ///
    /// [`create_conversation`]: Self::create_conversation
    pub fn create_conversation_with_id(
        &mut self,
        id: ConversationId,
        conversation: Conversation,
        config: Arc<AppConfig>,
    ) -> ConversationId {
        let _err = self
            .state
            .conversations
            .entry(id)
            .insert_entry(OnceLock::new())
            .get_mut()
            .set(Arc::new(RwLock::new(conversation)));

        let _err = self
            .state
            .events
            .entry(id)
            .insert_entry(OnceLock::new())
            .get_mut()
            .set(Arc::new(RwLock::new(
                ConversationStream::new(config).with_created_at(id.timestamp()),
            )));

        id
    }

    /// Create a new conversation and acquire an exclusive lock on it.
    ///
    /// Inserts into in-memory state and acquires an exclusive lock atomically,
    /// so no other process can claim the same conversation ID. For in-memory
    /// backends the lock has no cross-process backing but still provides the
    /// type-level mutation guarantee.
    pub fn create_and_lock_conversation(
        &mut self,
        conversation: Conversation,
        config: Arc<AppConfig>,
        session: Option<&Session>,
    ) -> Result<ConversationLock> {
        let id = self.create_conversation(conversation, config);
        self.lock_new_conversation(id, session)
    }

    /// Create a new conversation with a specific ID and acquire an exclusive
    /// lock on it.
    ///
    /// See [`create_and_lock_conversation`] for details.
    ///
    /// [`create_and_lock_conversation`]: Self::create_and_lock_conversation
    pub fn create_and_lock_conversation_with_id(
        &mut self,
        id: ConversationId,
        conversation: Conversation,
        config: Arc<AppConfig>,
        session: Option<&Session>,
    ) -> Result<ConversationLock> {
        self.create_conversation_with_id(id, conversation, config);
        self.lock_new_conversation(id, session)
    }

    /// Lock a just-created conversation.
    ///
    /// Returns an error if the lock cannot be acquired. A freshly created
    /// conversation should always be lockable — failure here indicates an
    /// infrastructure problem (e.g. a stale lock file from a crashed process).
    fn lock_new_conversation(
        &self,
        id: ConversationId,
        session: Option<&Session>,
    ) -> Result<ConversationLock> {
        let session_str = session.map(|s| s.id.as_str());
        let lock_guard = self
            .locker
            .try_lock(&id.to_string(), session_str)?
            .ok_or_else(|| Error::LockFailed(id.to_string()))?;

        let metadata = self
            .state
            .conversations
            .get(&id)
            .and_then(|cell| cell.get())
            .expect("just created")
            .clone();

        let events = self
            .state
            .events
            .get(&id)
            .and_then(|cell| cell.get())
            .expect("just created")
            .clone();

        let handle = ConversationHandle::new(id);
        Ok(ConversationLock::new(
            handle,
            metadata,
            events,
            Arc::clone(&self.persist),
            lock_guard,
        ))
    }

    /// Returns the globally unique ID of the workspace.
    #[must_use]
    pub fn id(&self) -> &Id {
        &self.id
    }

    /// Acquire a handle to a conversation, proving it exists in the index.
    pub fn acquire_conversation(&self, id: &ConversationId) -> Result<ConversationHandle> {
        if !self.state.conversations.contains_key(id) {
            return Err(Error::not_found("Conversation", id));
        }

        Ok(ConversationHandle::new(*id))
    }

    /// Get conversation metadata via a handle.
    ///
    /// Returns an error if the conversation data cannot be loaded from the
    /// backing store (e.g. the file was deleted or is corrupt).
    pub fn metadata(&self, h: &ConversationHandle) -> Result<RwLockReadGuard<'_, Conversation>> {
        let id = &h.id();
        let arc = self
            .state
            .conversations
            .get(id)
            .and_then(|cell| {
                maybe_init_conversation(&*self.loader, (id, cell));
                cell.get()
            })
            .ok_or_else(|| Error::not_found("Conversation metadata", id))?;
        Ok(arc.read())
    }

    /// Get the event stream via a handle.
    ///
    /// Returns an error if the conversation data cannot be loaded from the
    /// backing store (e.g. the file was deleted or is corrupt).
    pub fn events(
        &self,
        h: &ConversationHandle,
    ) -> Result<RwLockReadGuard<'_, ConversationStream>> {
        let id = &h.id();
        let arc = self
            .state
            .events
            .get(id)
            .and_then(|cell| {
                maybe_init_events(&*self.loader, (id, cell));
                cell.get()
            })
            .ok_or_else(|| Error::not_found("Conversation events", id))?;
        Ok(arc.read())
    }

    /// Acquire an exclusive cross-process lock on a conversation.
    ///
    /// Returns `Ok(LockResult::Acquired(lock))` if the lock was acquired, or
    /// `Ok(LockResult::AlreadyLocked(handle))` if another process holds it,
    /// giving the handle back so the caller can retry.
    ///
    /// Returns an error if conversation data cannot be loaded from the backing
    /// store (e.g. the user deleted a required file).
    pub fn lock_conversation(
        &self,
        handle: ConversationHandle,
        session: Option<&Session>,
    ) -> Result<LockResult> {
        let session_str = session.map(|s| s.id.as_str());
        let id = handle.id();

        let Some(lock_guard) = self.locker.try_lock(&id.to_string(), session_str)? else {
            return Ok(LockResult::AlreadyLocked(handle));
        };

        if let Some(cell) = self.state.conversations.get(&id) {
            maybe_init_conversation(&*self.loader, (&id, cell));
        }
        if let Some(cell) = self.state.events.get(&id) {
            maybe_init_events(&*self.loader, (&id, cell));
        }

        let metadata = self
            .state
            .conversations
            .get(&id)
            .and_then(|cell| cell.get())
            .ok_or_else(|| Error::not_found("Conversation metadata", &id))?
            .clone();

        let events = self
            .state
            .events
            .get(&id)
            .and_then(|cell| cell.get())
            .ok_or_else(|| Error::not_found("Conversation events", &id))?
            .clone();

        Ok(LockResult::Acquired(ConversationLock::new(
            handle,
            metadata,
            events,
            Arc::clone(&self.persist),
            lock_guard,
        )))
    }

    /// Archive a conversation, consuming its lock.
    ///
    /// Moves the conversation to the archive partition. The conversation is
    /// removed from the in-memory index and excluded from normal operations.
    pub fn archive_conversation(&mut self, mut conv: ConversationMut) {
        let id = conv.id();

        // Stamp archived_at and flush to disk before the rename.
        // If the rename fails, the conversation stays active with a stale
        // archived_at — a cosmetic issue, not data loss. Directory location
        // is the source of truth for archived state.
        conv.update_metadata(|m| m.archived_at = Some(chrono::Utc::now()));
        if let Err(e) = conv.flush() {
            warn!(%id, %e, "Failed to flush archived_at before archiving.");
        }
        conv.clear_dirty();

        if let Err(e) = self.persist.archive(&id) {
            warn!(%id, %e, "Failed to archive conversation.");
        }

        drop(conv);

        self.state.conversations.remove(&id);
        self.state.events.remove(&id);
    }

    /// Restore a conversation from the archive.
    ///
    /// Moves the conversation back to the active partition and inserts it into
    /// the in-memory index. Returns a handle for the restored conversation.
    pub fn unarchive_conversation(&mut self, id: &ConversationId) -> Result<ConversationHandle> {
        // Move out of .archive/ first, then clear archived_at through the
        // normal persist path. If the metadata write fails, the conversation
        // is active with a stale archived_at — cosmetic, not data loss.
        self.persist.unarchive(id)?;

        // Insert into the index so it can be loaded.
        self.state.conversations.entry(*id).or_default();
        self.state.events.entry(*id).or_default();

        // Clear archived_at and persist. The cells were just inserted above,
        // so init + get should succeed.
        let handle = ConversationHandle::new(*id);
        let meta_cell = &self.state.conversations[id];
        let events_cell = &self.state.events[id];
        maybe_init_conversation(&*self.loader, (id, meta_cell));
        maybe_init_events(&*self.loader, (id, events_cell));

        if let (Some(meta_arc), Some(events_arc)) = (meta_cell.get(), events_cell.get()) {
            meta_arc.write().archived_at = None;
            let meta = meta_arc.read();
            let events = events_arc.read();
            if let Err(e) = self.persist.write(id, &meta, &events) {
                warn!(%id, %e, "Failed to clear archived_at after unarchive.");
            }
        } else {
            warn!(%id, "Failed to load conversation after unarchive.");
        }

        Ok(handle)
    }

    /// Returns an iterator over archived conversations.
    ///
    /// Loads metadata for each archived conversation on demand. This performs
    /// I/O for every call — it is not cached in the workspace index.
    pub fn archived_conversations(
        &self,
    ) -> impl Iterator<Item = (ConversationId, Conversation)> + '_ {
        let ids = self
            .loader
            .load_conversation_ids(ConversationFilter { archived: true });

        ids.into_iter()
            .filter_map(|id| match self.loader.load_conversation_metadata(&id) {
                Ok(meta) => Some((id, meta)),
                Err(error) => {
                    warn!(%id, %error, "Failed to load archived conversation metadata.");
                    None
                }
            })
    }

    /// Remove a conversation, consuming its lock.
    pub fn remove_conversation_with_lock(&mut self, conv: ConversationMut) {
        let id = conv.id();
        conv.clear_dirty();

        if let Err(e) = self.persist.remove(&id) {
            warn!(%id, %e, "Failed to remove conversation from disk.");
        }

        drop(conv);

        self.state.conversations.remove(&id);
        self.state.events.remove(&id);
    }

    /// Read the lock holder info for a conversation.
    #[must_use]
    pub fn conversation_lock_info(&self, id: &ConversationId) -> Option<LockInfo> {
        self.locker.lock_info(&id.to_string())
    }

    fn ensure_all_metadata_loaded(&self) {
        let uninitialized: Vec<_> = self
            .state
            .conversations
            .iter()
            .filter(|(_, cell)| cell.get().is_none())
            .map(|(id, _)| *id)
            .collect();

        if uninitialized.is_empty() {
            return;
        }

        let loader = &self.loader;
        let loaded: Vec<_> = uninitialized
            .par_iter()
            .filter_map(|id| match loader.load_conversation_metadata(id) {
                Ok(meta) => Some((*id, meta)),
                Err(error) => {
                    warn!(%id, %error, "Failed to load conversation metadata.");
                    None
                }
            })
            .collect();

        for (id, meta) in loaded {
            if let Some(cell) = self.state.conversations.get(&id) {
                let _err = cell.set(Arc::new(RwLock::new(meta)));
            }
        }
    }

    /// Eagerly load every conversation's event stream in parallel.
    ///
    /// Intended for bulk consumers (e.g. workspace-wide grep) that would
    /// otherwise pay for N sequential disk reads via [`Self::events`].
    /// Already-loaded streams are left untouched, so calling this repeatedly
    /// is cheap.
    pub fn ensure_all_events_loaded(&self) {
        let uninitialized: Vec<_> = self
            .state
            .events
            .iter()
            .filter(|(_, cell)| cell.get().is_none())
            .map(|(id, _)| *id)
            .collect();

        if uninitialized.is_empty() {
            return;
        }

        let loader = &self.loader;
        let loaded: Vec<_> = uninitialized
            .par_iter()
            .filter_map(|id| match loader.load_conversation_stream(id) {
                Ok(stream) => Some((*id, stream)),
                Err(error) => {
                    warn!(%id, %error, "Failed to load conversation events.");
                    None
                }
            })
            .collect();

        for (id, stream) in loaded {
            if let Some(cell) = self.state.events.get(&id) {
                let _err = cell.set(Arc::new(RwLock::new(stream)));
            }
        }
    }
}

#[cfg(debug_assertions)]
impl Workspace {
    /// Create a [`ConversationLock`] without a cross-process flock for tests.
    #[doc(hidden)]
    #[must_use]
    pub fn test_lock(&self, handle: ConversationHandle) -> ConversationLock {
        use jp_storage::backend::NoopLockGuard;

        let id = handle.id();

        if let Some(cell) = self.state.conversations.get(&id) {
            maybe_init_conversation(&*self.loader, (&id, cell));
        }
        if let Some(cell) = self.state.events.get(&id) {
            maybe_init_events(&*self.loader, (&id, cell));
        }

        let metadata = self
            .state
            .conversations
            .get(&id)
            .and_then(|cell| cell.get())
            .expect("test_lock: metadata not found")
            .clone();

        let events = self
            .state
            .events
            .get(&id)
            .and_then(|cell| cell.get())
            .expect("test_lock: events not found")
            .clone();

        ConversationLock::new(
            handle,
            metadata,
            events,
            Arc::clone(&self.persist),
            Box::new(NoopLockGuard),
        )
    }
}

fn maybe_init_conversation(
    loader: &dyn LoadBackend,
    (id, cell): (&ConversationId, &OnceLock<Arc<RwLock<Conversation>>>),
) {
    if cell.get().is_some() {
        return;
    }

    let Ok(meta) = loader.load_conversation_metadata(id) else {
        warn!(%id, "Failed to load conversation metadata. Skipping.");
        return;
    };

    if let Err(error) = cell.set(Arc::new(RwLock::new(meta))) {
        warn!(%id, ?error, "Failed to initialize conversation metadata. Skipping.");
    }
}

fn maybe_init_events(
    loader: &dyn LoadBackend,
    (id, cell): (&ConversationId, &OnceLock<Arc<RwLock<ConversationStream>>>),
) {
    if cell.get().is_some() {
        return;
    }

    let Ok(stream) = loader.load_conversation_stream(id) else {
        warn!(%id, "Failed to load conversation events. Skipping.");
        return;
    };

    if let Err(error) = cell.set(Arc::new(RwLock::new(stream))) {
        warn!(%id, ?error, "Failed to initialize conversation events. Skipping.");
    }
}

pub fn user_data_dir() -> Result<Utf8PathBuf> {
    directories::ProjectDirs::from("", "", APPLICATION)
        .ok_or(Error::MissingHome)?
        .data_local_dir()
        .to_path_buf()
        .try_into()
        .map_err(FromPathBufError::into_io_error)
        .map_err(Into::into)
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
