//! JP Workspace: A crate for managing LLM-assisted code conversations
//!
//! This crate provides data models and storage operations for the JP workspace,
//! a CLI tool for managing LLM-assisted code conversations with fine-grained
//! control over context and behavior.

mod conversation_lock;
mod error;
mod handle;
mod id;
pub mod persist;
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
use jp_storage::{Storage, lock::LockInfo};
use parking_lot::{ArcRwLockReadGuard, RawRwLock, RwLock, RwLockReadGuard};
use persist::FsPersistBackend;
use rayon::prelude::*;
pub use sanitize::{SanitizeReport, TrashedConversation};
use state::State;
use tracing::{debug, trace, warn};

use crate::{persist::PersistBackend, session::Session};

const APPLICATION: &str = "jp";

#[derive(Debug)]
pub struct Workspace {
    /// The root directory of the workspace.
    root: Utf8PathBuf,

    /// The globally unique ID of the workspace.
    id: id::Id,

    /// The (optional) storage for the workspace.
    storage: Option<Storage>,

    /// The in-memory state of the workspace.
    state: State,

    /// Disable persistence for the workspace.
    disable_persistence: bool,

    /// The persist backend for writing conversations to disk.
    ///
    /// Built lazily from `Storage` when `load_conversation_index` is called,
    /// or when `persisted_at` configures storage. Shared with
    /// `ConversationLock` / `ConversationMut` instances.
    persist_backend: Option<Arc<dyn PersistBackend>>,
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
    pub fn new(root: impl Into<Utf8PathBuf>) -> Self {
        Self::new_with_id(root, id::Id::new())
    }

    /// Creates a new workspace with the given root directory and ID.
    pub fn new_with_id(root: impl Into<Utf8PathBuf>, id: id::Id) -> Self {
        let root = root.into();
        trace!(root = %root, id = %id, "Initializing Workspace.");

        Self {
            root,
            id,
            storage: None,
            state: State::default(),
            disable_persistence: false,
            persist_backend: None,
        }
    }

    /// Get the root path of the workspace.
    #[must_use]
    pub fn root(&self) -> &Utf8Path {
        &self.root
    }

    /// Enable persistence for the workspace at the given (absolute) path.
    pub fn persisted_at(mut self, path: &Utf8Path) -> Result<Self> {
        trace!(path = %path, "Enabling workspace persistence.");

        self.disable_persistence = false;
        self.storage = Some(Storage::new(path)?);
        self.rebuild_persist_backend();
        Ok(self)
    }

    /// Enable local storage for the workspace.
    pub fn with_local_storage(mut self) -> Result<Self> {
        trace!("Enabling local storage.");

        if self.storage.is_none() {
            return Err(Error::MissingStorage);
        }

        let root = user_data_dir()?.join("workspace");
        let id: &str = &self.id;
        let name = self
            .root
            .file_name()
            .ok_or_else(|| Error::NotDir(self.root.clone()))?;

        self.storage = self
            .storage
            .take()
            .map(|storage| storage.with_user_storage(&root, name, id))
            .transpose()?;

        self.rebuild_persist_backend();
        trace!("Local storage enabled.");

        Ok(self)
    }

    /// Enable local storage at an explicit path, for testing.
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn with_local_storage_at(mut self, root: &Utf8Path, name: &str, id: &str) -> Result<Self> {
        self.storage = self
            .storage
            .take()
            .map(|storage| storage.with_user_storage(root, name, id))
            .transpose()?;

        self.rebuild_persist_backend();
        Ok(self)
    }

    /// Disable persistence for the workspace.
    pub fn disable_persistence(&mut self) {
        self.disable_persistence = true;
    }

    /// Returns the path to the storage directory, if persistence is enabled.
    #[must_use]
    pub fn storage_path(&self) -> Option<&Utf8Path> {
        self.storage.as_ref().map(Storage::path)
    }

    /// Returns the path to the user storage directory, if persistence is
    /// enabled, and user storage is configured.
    #[must_use]
    pub fn user_storage_path(&self) -> Option<&Utf8Path> {
        self.storage.as_ref().and_then(Storage::user_storage_path)
    }

    /// Scan conversation IDs from disk and populate the workspace index.
    ///
    /// All entries are lazily initialized — metadata and events are loaded from
    /// disk on first access via [`metadata`], [`events`], etc.
    ///
    /// Call [`sanitize`](Self::sanitize) before this method to ensure the
    /// filesystem is in a consistent state.
    ///
    /// This call is a no-op if the workspace has no backing storage.
    ///
    /// [`metadata`]: Self::metadata
    /// [`events`]: Self::events
    pub fn load_conversation_index(&mut self) {
        let Some(storage) = self.storage.as_ref() else {
            return;
        };

        trace!("Loading conversation index.");
        let conversation_ids = storage.load_all_conversation_ids();

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

        self.rebuild_persist_backend();
    }

    /// Eagerly load a single conversation's metadata and events from disk.
    pub fn eager_load_conversation(&mut self, h: &ConversationHandle) -> Result<()> {
        let storage = self.storage.as_ref().ok_or(Error::MissingStorage)?;
        let id = &h.id();

        if let Some(cell) = self.state.conversations.get(id)
            && cell.get().is_none()
        {
            let metadata = storage.load_conversation_metadata(id)?;
            let _err = cell.set(Arc::new(RwLock::new(metadata)));
        }

        if let Some(cell) = self.state.events.get(id)
            && cell.get().is_none()
        {
            let stream = storage.load_conversation_stream(id)?;
            let _err = cell.set(Arc::new(RwLock::new(stream)));
        }

        Ok(())
    }

    pub fn remove_ephemeral_conversations(&mut self, skip: &[ConversationId]) {
        if self.disable_persistence {
            return;
        }

        let Some(storage) = self.storage.as_mut() else {
            return;
        };

        storage.remove_ephemeral_conversations(skip);
    }

    /// Returns an iterator over all conversations.
    ///
    /// Uninitialized metadata is loaded from disk in parallel (via rayon)
    /// on first access. Already-loaded conversations are returned as-is.
    ///
    /// Each item yields a read guard that auto-derefs to `&Conversation`.
    /// Do **not** hold these guards across `.await` points.
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
    /// Returns the conversation ID. No data is written to disk and no
    /// cross-process lock is acquired. Persistence happens when a
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
    /// Inserts into in-memory state and acquires the cross-process flock
    /// atomically, so no other process can claim the same conversation ID.
    /// If storage is not configured, the lock has no flock backing but
    /// still provides the type-level mutation guarantee.
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
    /// Acquires the flock if storage is configured, otherwise returns a lock
    /// without flock backing (for in-memory workspaces).
    fn lock_new_conversation(
        &self,
        id: ConversationId,
        session: Option<&Session>,
    ) -> Result<ConversationLock> {
        let file_lock = self
            .storage
            .as_ref()
            .map(|s| s.try_lock_conversation(&id.to_string(), session.map(|s| s.id.as_str())))
            .transpose()?
            .flatten();

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

        let writer = if self.disable_persistence {
            None
        } else {
            self.persist_backend.clone()
        };

        let handle = ConversationHandle::new(id);
        Ok(ConversationLock::new(
            handle, metadata, events, writer, file_lock,
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
    /// Returns an error if the conversation data cannot be loaded from disk
    /// (e.g. the file was deleted or is corrupt).
    pub fn metadata(&self, h: &ConversationHandle) -> Result<RwLockReadGuard<'_, Conversation>> {
        let id = &h.id();
        let arc = self
            .state
            .conversations
            .get(id)
            .and_then(|cell| {
                maybe_init_conversation(self.storage.as_ref(), (id, cell));
                cell.get()
            })
            .ok_or_else(|| Error::not_found("Conversation metadata", id))?;
        Ok(arc.read())
    }

    /// Get the event stream via a handle.
    ///
    /// Returns an error if the conversation data cannot be loaded from disk
    /// (e.g. the file was deleted or is corrupt).
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
                maybe_init_events(self.storage.as_ref(), (id, cell));
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
    /// Returns an error if conversation data cannot be loaded from disk (e.g.
    /// the user deleted a required file).
    pub fn lock_conversation(
        &self,
        handle: ConversationHandle,
        session: Option<&Session>,
    ) -> Result<LockResult> {
        let session = session.map(|s| s.id.as_str());
        let storage = self.storage.as_ref().ok_or(Error::MissingStorage)?;
        let id = handle.id();

        let Some(file_lock) = storage.try_lock_conversation(&id.to_string(), session)? else {
            return Ok(LockResult::AlreadyLocked(handle));
        };

        if let Some(cell) = self.state.conversations.get(&id) {
            maybe_init_conversation(self.storage.as_ref(), (&id, cell));
        }
        if let Some(cell) = self.state.events.get(&id) {
            maybe_init_events(self.storage.as_ref(), (&id, cell));
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

        let writer = if self.disable_persistence {
            None
        } else {
            self.persist_backend.clone()
        };

        Ok(LockResult::Acquired(ConversationLock::new(
            handle,
            metadata,
            events,
            writer,
            Some(file_lock),
        )))
    }

    /// Remove a conversation, consuming its lock.
    pub fn remove_conversation_with_lock(&mut self, conv: ConversationMut) {
        let id = conv.id();
        conv.clear_dirty();

        if let Some(backend) = &self.persist_backend
            && let Err(e) = backend.remove(&id)
        {
            warn!(%id, %e, "Failed to remove conversation from disk.");
        }

        drop(conv);

        self.state.conversations.remove(&id);
        self.state.events.remove(&id);
    }

    /// Read the lock holder info for a conversation.
    #[must_use]
    pub fn conversation_lock_info(&self, id: &ConversationId) -> Option<LockInfo> {
        let storage = self.storage.as_ref()?;
        storage.read_conversation_lock_info(&id.to_string())
    }

    /// Create a [`ConversationLock`] without a cross-process flock for tests.
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    #[must_use]
    pub fn test_lock(&self, handle: ConversationHandle) -> ConversationLock {
        let id = handle.id();

        if let Some(cell) = self.state.conversations.get(&id) {
            maybe_init_conversation(self.storage.as_ref(), (&id, cell));
        }
        if let Some(cell) = self.state.events.get(&id) {
            maybe_init_events(self.storage.as_ref(), (&id, cell));
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

        let writer = self.persist_backend.clone();
        ConversationLock::new(handle, metadata, events, writer, None)
    }

    fn rebuild_persist_backend(&mut self) {
        self.persist_backend = self
            .storage
            .as_ref()
            .map(|s| Arc::new(FsPersistBackend::from_storage(s)) as Arc<_>);
    }

    fn ensure_all_metadata_loaded(&self) {
        let Some(storage) = self.storage.as_ref() else {
            return;
        };

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

        let loaded: Vec<_> = uninitialized
            .par_iter()
            .filter_map(|id| match storage.load_conversation_metadata(id) {
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
}

fn maybe_init_conversation(
    storage: Option<&Storage>,
    (id, cell): (&ConversationId, &OnceLock<Arc<RwLock<Conversation>>>),
) {
    let Some(storage) = storage else {
        return;
    };

    if cell.get().is_none() {
        let Ok(meta) = storage.load_conversation_metadata(id) else {
            warn!(%id, "Failed to load conversation metadata. Skipping.");
            return;
        };

        if let Err(error) = cell.set(Arc::new(RwLock::new(meta))) {
            warn!(%id, ?error, "Failed to initialize conversation metadata. Skipping.");
        }
    }
}

fn maybe_init_events(
    storage: Option<&Storage>,
    (id, cell): (&ConversationId, &OnceLock<Arc<RwLock<ConversationStream>>>),
) {
    let Some(storage) = storage else {
        return;
    };

    if cell.get().is_none() {
        let Ok(stream) = storage.load_conversation_stream(id) else {
            warn!(%id, "Failed to load conversation events. Skipping.");
            return;
        };

        if let Err(error) = cell.set(Arc::new(RwLock::new(stream))) {
            warn!(%id, ?error, "Failed to initialize conversation events. Skipping.");
        }
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
