//! Null/no-op backend implementations.
//!
//! [`NullPersistBackend`] silently discards all writes — used for ephemeral
//! mode (`--no-persist`) and error-path persistence suppression.
//!
//! [`NullLockBackend`] is a lock backend where every lock attempt succeeds
//! immediately — used alongside `NullPersistBackend` for `--no-persist` so that
//! ephemeral queries never block on lock contention.
//!
//! [`NoopLockGuard`] is a lock guard that does nothing on drop — used by
//! `NullLockBackend` and the `test_lock` helper.

use jp_conversation::{Conversation, ConversationId, ConversationStream};

use super::{ConversationLockGuard, LockBackend, PersistBackend};
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
    ) -> Result<()> {
        Ok(())
    }

    fn remove(&self, _id: &ConversationId) -> Result<()> {
        Ok(())
    }
}

/// A [`LockBackend`] where every lock attempt succeeds immediately.
///
/// No cross-process or in-process exclusion is enforced. Used for
/// `--no-persist` mode where no data is written to disk, so lock contention is
/// irrelevant.
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
