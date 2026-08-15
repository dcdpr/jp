//! Null/no-op backend implementations.
//!
//! [`NullPersistBackend`] silently discards all writes — used for ephemeral
//! mode (`--no-persist`) and error-path persistence suppression.
//!
//! [`NullLockBackend`] is a lock backend where every lock attempt succeeds
//! immediately — used alongside `NullPersistBackend` for `--no-persist` so
//! that ephemeral queries never block on lock contention.
//!
//! [`NoopLockGuard`] is a lock guard that does nothing on drop — used by
//! `NullLockBackend` and the `test_lock` helper.
//!
//! [`ReadOnlySessionBackend`] serves session reads from an inner backend and
//! discards session writes — used for `--no-persist` so an ephemeral run can
//! still tell which conversation the session is working on without changing it.

use std::sync::Arc;

use jp_conversation::{Conversation, ConversationId, ConversationStream};
use serde_json::Value;

use super::{ConversationLockGuard, LockBackend, PersistBackend, Projection, SessionBackend};
use crate::{error::Result, lock::LockInfo};

/// A [`PersistBackend`] that silently discards all writes.
#[derive(Debug)]
pub struct NullPersistBackend;

impl PersistBackend for NullPersistBackend {
    fn write(
        &self,
        _id: &ConversationId,
        _metadata: &Conversation,
        _events: &ConversationStream,
        _projection: Projection,
    ) -> Result<()> {
        Ok(())
    }

    fn remove(&self, _id: &ConversationId) -> Result<()> {
        Ok(())
    }

    fn archive(&self, _id: &ConversationId) -> Result<()> {
        Ok(())
    }

    fn unarchive(&self, _id: &ConversationId) -> Result<()> {
        Ok(())
    }
}

/// A [`LockBackend`] where every lock attempt succeeds immediately.
///
/// No cross-process or in-process exclusion is enforced.
/// Used for `--no-persist` mode where no data is written to disk, so lock
/// contention is irrelevant.
#[derive(Debug)]
pub struct NullLockBackend;

impl LockBackend for NullLockBackend {
    fn try_lock(
        &self,
        _conversation_id: &str,
        _session: Option<&str>,
    ) -> Result<Option<Box<dyn ConversationLockGuard>>> {
        Ok(Some(Box::new(NoopLockGuard)))
    }

    fn lock_info(&self, _conversation_id: &str) -> Option<LockInfo> {
        None
    }

    fn list_orphaned_locks(&self) -> Vec<ConversationId> {
        vec![]
    }
}

/// A [`ConversationLockGuard`] that does nothing on drop.
#[derive(Debug)]
pub struct NoopLockGuard;

impl ConversationLockGuard for NoopLockGuard {}

/// A [`SessionBackend`] that reads through to an inner backend and drops every
/// write.
///
/// Reads stay live because resolving "which conversation is this session on" is
/// a read; recording a *new* active conversation is a write, and a run that
/// leaves nothing on disk must not leave the session pointing at a conversation
/// that was never persisted.
#[derive(Debug)]
pub struct ReadOnlySessionBackend(Arc<dyn SessionBackend>);

impl ReadOnlySessionBackend {
    /// Wrap `inner`, serving its reads and discarding its writes.
    #[must_use]
    pub fn new(inner: Arc<dyn SessionBackend>) -> Self {
        Self(inner)
    }
}

impl SessionBackend for ReadOnlySessionBackend {
    fn load_session(&self, session_key: &str) -> Result<Option<Value>> {
        self.0.load_session(session_key)
    }

    fn save_session(&self, _session_key: &str, _data: &Value) -> Result<()> {
        Ok(())
    }

    fn list_session_keys(&self) -> Vec<String> {
        self.0.list_session_keys()
    }
}

#[cfg(test)]
#[path = "null_tests.rs"]
mod tests;
