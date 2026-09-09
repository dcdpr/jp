//! Locking backend trait for conversation-level exclusive access.

use std::fmt::Debug;

use jp_conversation::ConversationId;

use crate::{lock::LockInfo, resource_lock::ResourceGuard};

/// Conversation-level locking.
///
/// Abstracts over the locking mechanism.
/// Filesystem backends use `flock`; in-memory backends use in-process mutexes.
pub trait LockBackend: Send + Sync + Debug {
    /// Attempt to acquire an exclusive lock on a conversation.
    ///
    /// Returns `Ok(Some(guard))` if acquired, `Ok(None)` if another holder has
    /// it, or `Err` on infrastructure failure.
    fn try_lock(
        &self,
        conversation_id: &str,
        session: Option<&str>,
    ) -> crate::error::Result<Option<Box<dyn ConversationLockGuard>>>;

    /// Read diagnostic info about a lock holder.
    fn lock_info(&self, conversation_id: &str) -> Option<LockInfo>;

    /// List conversation IDs with orphaned locks (not held by any process).
    fn list_orphaned_locks(&self) -> Vec<ConversationId>;
}

/// A held conversation lock.
/// Released on drop.
pub trait ConversationLockGuard: Send + Sync + Debug {}

/// Any held resource lock can serve as a conversation lock guard; the backends
/// produce their guards through [`ResourceLocker`] implementations.
///
/// [`ResourceLocker`]: crate::resource_lock::ResourceLocker
impl ConversationLockGuard for Box<dyn ResourceGuard> {}
