//! In-memory storage backend.
//!
//! [`InMemoryStorageBackend`] provides a filesystem-free implementation of all
//! four backend traits. Intended for tests and future non-filesystem
//! environments.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Utc};
use jp_conversation::{Conversation, ConversationId, ConversationStream};
use serde_json::Value;

use super::{
    ConversationLockGuard, LoadBackend, LockBackend, PersistBackend, SanitizeReport, SessionBackend,
};
use crate::{LoadError, error::Result, load::LoadErrorInner, lock::LockInfo};

/// Purely in-memory storage backend.
///
/// All data lives in process memory behind mutexes. No filesystem access.
/// Locking uses in-process checks (not cross-process `flock`).
#[derive(Debug, Default, Clone)]
pub struct InMemoryStorageBackend {
    conversations: Arc<Mutex<HashMap<ConversationId, (Conversation, ConversationStream)>>>,
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
    ) -> Result<()> {
        self.conversations
            .lock()
            .expect("poisoned")
            .insert(*id, (metadata.clone(), events.clone()));
        Ok(())
    }

    fn remove(&self, id: &ConversationId) -> Result<()> {
        self.conversations.lock().expect("poisoned").remove(id);
        Ok(())
    }
}

impl LoadBackend for InMemoryStorageBackend {
    fn load_all_conversation_ids(&self) -> Vec<ConversationId> {
        let mut ids: Vec<_> = self
            .conversations
            .lock()
            .expect("poisoned")
            .keys()
            .copied()
            .collect();
        ids.sort();
        ids
    }

    fn load_conversation_metadata(
        &self,
        id: &ConversationId,
    ) -> std::result::Result<Conversation, LoadError> {
        self.conversations
            .lock()
            .expect("poisoned")
            .get(id)
            .map(|(meta, _)| meta.clone())
            .ok_or_else(|| {
                LoadError::new(
                    format!("<memory>/{id}"),
                    LoadErrorInner::MissingConversationMetadata(*id),
                )
            })
    }

    fn load_conversation_stream(
        &self,
        id: &ConversationId,
    ) -> std::result::Result<ConversationStream, LoadError> {
        self.conversations
            .lock()
            .expect("poisoned")
            .get(id)
            .map(|(_, events)| events.clone())
            .ok_or_else(|| {
                LoadError::new(
                    format!("<memory>/{id}"),
                    LoadErrorInner::MissingConversationStream(*id),
                )
            })
    }

    fn load_expired_conversation_ids(&self, now: DateTime<Utc>) -> Vec<ConversationId> {
        self.conversations
            .lock()
            .expect("poisoned")
            .iter()
            .filter_map(|(id, (meta, _))| {
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

/// A held in-process lock. Removes itself from the lock set on drop.
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
