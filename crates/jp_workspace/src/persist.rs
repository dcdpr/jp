//! Persistence backend abstraction for conversation data.
//!
//! `PersistBackend` decouples conversation persistence from the storage
//! mechanism. The production implementation (`FsPersistBackend`) delegates to
//! `jp_storage::Storage`; tests can use `MockPersistBackend` to capture and
//! assert against persist calls without disk I/O.

use jp_conversation::{Conversation, ConversationId, ConversationStream};
use jp_storage::Storage;

use crate::error::Result;

/// Abstraction over conversation persistence.
///
/// Implementations write a single conversation's metadata and events to some
/// backend (filesystem, mock, etc.).
pub trait PersistBackend: Send + Sync + std::fmt::Debug {
    /// Persist a conversation's metadata and events.
    fn write(
        &self,
        id: &ConversationId,
        metadata: &Conversation,
        events: &ConversationStream,
    ) -> Result<()>;

    /// Remove a conversation's persisted data entirely.
    fn remove(&self, id: &ConversationId) -> Result<()>;
}

/// Filesystem-backed persistence that delegates to [`Storage`].
///
/// Constructed from a `Storage` reference at workspace initialization time. The
/// `Storage` paths are captured so persistence can be invoked from
/// `ConversationMut::Drop` without requiring a live `Storage` reference.
#[derive(Debug)]
pub struct FsPersistBackend {
    storage: Storage,
}

impl FsPersistBackend {
    /// Build from a `Storage` instance by cloning it.
    pub(crate) fn from_storage(storage: &Storage) -> Self {
        Self {
            storage: storage.clone(),
        }
    }
}

impl PersistBackend for FsPersistBackend {
    fn write(
        &self,
        id: &ConversationId,
        metadata: &Conversation,
        events: &ConversationStream,
    ) -> Result<()> {
        self.storage
            .persist_conversation(id, metadata, events)
            .map_err(Into::into)
    }

    fn remove(&self, id: &ConversationId) -> Result<()> {
        self.storage.remove_conversation(id).map_err(Into::into)
    }
}

/// Mock persistence backend for tests.
///
/// Records all `write` calls so tests can assert on what was persisted.
#[cfg(debug_assertions)]
#[derive(Debug, Default)]
pub struct MockPersistBackend {
    writes: std::sync::Mutex<Vec<(ConversationId, Conversation, ConversationStream)>>,
    removes: std::sync::Mutex<Vec<ConversationId>>,
}

#[cfg(debug_assertions)]
impl MockPersistBackend {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a snapshot of all write calls.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn writes(&self) -> Vec<(ConversationId, Conversation, ConversationStream)> {
        self.writes.lock().unwrap().clone()
    }

    /// Returns a snapshot of all remove calls.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[must_use]
    pub fn removes(&self) -> Vec<ConversationId> {
        self.removes.lock().unwrap().clone()
    }
}

#[cfg(debug_assertions)]
impl PersistBackend for MockPersistBackend {
    fn write(
        &self,
        id: &ConversationId,
        metadata: &Conversation,
        events: &ConversationStream,
    ) -> Result<()> {
        self.writes
            .lock()
            .unwrap()
            .push((*id, metadata.clone(), events.clone()));
        Ok(())
    }

    fn remove(&self, id: &ConversationId) -> Result<()> {
        self.removes.lock().unwrap().push(*id);
        Ok(())
    }
}
