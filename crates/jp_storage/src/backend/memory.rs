//! In-memory storage backend.
//!
//! [`InMemoryStorageBackend`] provides a filesystem-free implementation of all
//! four backend traits.
//! Intended for tests and future non-filesystem environments.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Utc};
use jp_conversation::{Conversation, ConversationId, ConversationStream};
use serde_json::Value;

use super::{
    ConversationFilter, ConversationIndexEntry, ConversationLockGuard, LoadBackend, LockBackend,
    PersistBackend, Projection, SanitizeReport, SessionBackend, StoragePresence,
};
use crate::{LoadError, error::Result, load::LoadErrorInner, lock::LockInfo};

/// A stored conversation: metadata, event stream, and the projection of its
/// most recent write.
type StoredConversation = (Conversation, ConversationStream, Projection);

/// Purely in-memory storage backend.
///
/// All data lives in process memory behind mutexes.
/// No filesystem access.
/// Locking uses in-process checks (not cross-process `flock`).
#[derive(Debug, Default, Clone)]
pub struct InMemoryStorageBackend {
    conversations: Arc<Mutex<HashMap<ConversationId, StoredConversation>>>,
    archived: Arc<Mutex<HashMap<ConversationId, StoredConversation>>>,
    locks: Arc<Mutex<HashSet<String>>>,
    sessions: Arc<Mutex<HashMap<String, Value>>>,
}

impl InMemoryStorageBackend {
    /// Create a new empty in-memory backend.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl PersistBackend for InMemoryStorageBackend {
    fn write(
        &self,
        id: &ConversationId,
        metadata: &Conversation,
        events: &ConversationStream,
        projection: Projection,
    ) -> Result<()> {
        self.conversations
            .lock()
            .expect("poisoned")
            .insert(*id, (metadata.clone(), events.clone(), projection));
        Ok(())
    }

    fn remove(&self, id: &ConversationId) -> Result<()> {
        self.conversations.lock().expect("poisoned").remove(id);
        self.archived.lock().expect("poisoned").remove(id);
        Ok(())
    }

    fn archive(&self, id: &ConversationId) -> Result<()> {
        let entry = self.conversations.lock().expect("poisoned").remove(id);
        match entry {
            Some(entry) => {
                self.archived.lock().expect("poisoned").insert(*id, entry);
                Ok(())
            }
            None => Err(crate::Error::ConversationNotFound(*id)),
        }
    }

    fn unarchive(&self, id: &ConversationId) -> Result<()> {
        let entry = self.archived.lock().expect("poisoned").remove(id);
        match entry {
            Some(entry) => {
                self.conversations
                    .lock()
                    .expect("poisoned")
                    .insert(*id, entry);
                Ok(())
            }
            None => Err(crate::Error::ConversationNotFound(*id)),
        }
    }
}

impl LoadBackend for InMemoryStorageBackend {
    fn load_conversation_index(&self, filter: ConversationFilter) -> Vec<ConversationIndexEntry> {
        let store = if filter.archived {
            &self.archived
        } else {
            &self.conversations
        };

        // Single-store backend: presence is derived from the projection
        // recorded at the conversation's last write, since there is no second
        // root to project into.
        let mut entries: Vec<_> = store
            .lock()
            .expect("poisoned")
            .iter()
            .map(|(id, (_, _, projection))| ConversationIndexEntry {
                id: *id,
                presence: StoragePresence::from(*projection),
            })
            .collect();
        entries.sort_by_key(|entry| entry.id);
        entries
    }

    fn load_conversation_metadata(
        &self,
        id: &ConversationId,
    ) -> std::result::Result<Conversation, LoadError> {
        let convs = self.conversations.lock().expect("poisoned");
        if let Some((meta, _, _)) = convs.get(id) {
            return Ok(meta.clone());
        }
        drop(convs);

        let archived = self.archived.lock().expect("poisoned");
        if let Some((meta, _, _)) = archived.get(id) {
            return Ok(meta.clone());
        }
        drop(archived);

        Err(LoadError::new(
            format!("<memory>/{id}"),
            LoadErrorInner::MissingConversationMetadata(*id),
        ))
    }

    fn load_conversation_stream(
        &self,
        id: &ConversationId,
    ) -> std::result::Result<ConversationStream, LoadError> {
        let convs = self.conversations.lock().expect("poisoned");
        if let Some((_, events, _)) = convs.get(id) {
            return Ok(events.clone());
        }
        drop(convs);

        let archived = self.archived.lock().expect("poisoned");
        if let Some((_, events, _)) = archived.get(id) {
            return Ok(events.clone());
        }
        drop(archived);

        Err(LoadError::new(
            format!("<memory>/{id}"),
            LoadErrorInner::MissingConversationStream(*id),
        ))
    }

    fn load_expired_conversation_ids(&self, now: DateTime<Utc>) -> Vec<ConversationId> {
        self.conversations
            .lock()
            .expect("poisoned")
            .iter()
            .filter_map(|(id, (meta, _, _))| {
                let expires_at = meta.expires_at?;
                (expires_at <= now).then_some(*id)
            })
            .collect()
    }

    fn sanitize(&self) -> Result<SanitizeReport> {
        // In-memory data is always structurally valid.
        Ok(SanitizeReport::default())
    }
}

impl LockBackend for InMemoryStorageBackend {
    fn try_lock(
        &self,
        conversation_id: &str,
        _session: Option<&str>,
    ) -> Result<Option<Box<dyn ConversationLockGuard>>> {
        let mut locks = self.locks.lock().expect("poisoned");
        if locks.contains(conversation_id) {
            return Ok(None);
        }
        locks.insert(conversation_id.to_owned());

        Ok(Some(Box::new(InMemoryLockGuard {
            conversation_id: conversation_id.to_owned(),
            locks: Arc::clone(&self.locks),
        })))
    }

    fn lock_info(&self, _conversation_id: &str) -> Option<LockInfo> {
        // In-memory locks have no diagnostic metadata.
        None
    }

    fn list_orphaned_locks(&self) -> Vec<ConversationId> {
        // In-memory locks are released on drop; no orphans possible.
        vec![]
    }
}

impl SessionBackend for InMemoryStorageBackend {
    fn load_session(&self, session_key: &str) -> Result<Option<Value>> {
        let sessions = self.sessions.lock().expect("poisoned");
        Ok(sessions.get(session_key).cloned())
    }

    fn save_session(&self, session_key: &str, data: &Value) -> Result<()> {
        self.sessions
            .lock()
            .expect("poisoned")
            .insert(session_key.to_owned(), data.clone());
        Ok(())
    }

    fn list_session_keys(&self) -> Vec<String> {
        self.sessions
            .lock()
            .expect("poisoned")
            .keys()
            .cloned()
            .collect()
    }
}

/// A held in-process lock.
/// Removes itself from the lock set on drop.
struct InMemoryLockGuard {
    conversation_id: String,
    locks: Arc<Mutex<HashSet<String>>>,
}

impl Drop for InMemoryLockGuard {
    fn drop(&mut self) {
        self.locks
            .lock()
            .expect("poisoned")
            .remove(&self.conversation_id);
    }
}

impl fmt::Debug for InMemoryLockGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InMemoryLockGuard")
            .field("conversation_id", &self.conversation_id)
            .finish_non_exhaustive()
    }
}

impl ConversationLockGuard for InMemoryLockGuard {}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
