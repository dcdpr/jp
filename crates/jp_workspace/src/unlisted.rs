//! Conversations the store holds that the conversation index leaves out.
//!
//! A conversation that fails startup validation, or whose directory holds none
//! of the conversation files, is never indexed, so it cannot be reached through
//! a [`ConversationHandle`].
//! [`Workspace::unlisted_conversation`] reads what the store holds for one by
//! ID, and [`Workspace::lock_unlisted_conversation`] and
//! [`Workspace::remove_unlisted_conversation`] remove it, through the same
//! backends as every other conversation.
//!
//! [`ConversationHandle`]: crate::ConversationHandle

use jp_conversation::ConversationId;
use jp_storage::backend::{ConversationLockGuard, UnlistedConversation};

use crate::{
    Workspace,
    error::{Error, Result},
};

/// A held lock on an unlisted conversation, with its stored state as read while
/// the lock was held.
#[derive(Debug)]
pub struct UnlistedLock {
    conversation: UnlistedConversation,
    _guard: Box<dyn ConversationLockGuard>,
}

impl UnlistedLock {
    #[must_use]
    pub fn conversation(&self) -> &UnlistedConversation {
        &self.conversation
    }
}

impl Workspace {
    /// What the store holds for conversation `id` when the conversation index
    /// leaves it out.
    ///
    /// `None` for a conversation in the index, and for one the store holds
    /// nothing for.
    #[must_use]
    pub fn unlisted_conversation(&self, id: &ConversationId) -> Option<UnlistedConversation> {
        if self.state.conversations.contains_key(id) {
            return None;
        }

        self.loader.unlisted_conversation(id)
    }

    /// Lock unlisted conversation `id`, and read what the store holds for it
    /// while the lock is held.
    ///
    /// Returns `Ok(None)` when another process holds the lock.
    ///
    /// # Errors
    ///
    /// Fails when the lock cannot be taken, or when the store no longer holds
    /// the conversation once it is.
    pub fn lock_unlisted_conversation(&self, id: &ConversationId) -> Result<Option<UnlistedLock>> {
        let Some(guard) = self.locker.try_lock(&id.to_string(), None)? else {
            return Ok(None);
        };

        let conversation = self
            .unlisted_conversation(id)
            .ok_or_else(|| Error::not_found("Conversation", id))?;

        Ok(Some(UnlistedLock {
            conversation,
            _guard: guard,
        }))
    }

    /// Remove an unlisted conversation, consuming its lock.
    ///
    /// # Errors
    ///
    /// Fails when the store cannot remove the conversation.
    pub fn remove_unlisted_conversation(&self, lock: UnlistedLock) -> Result<()> {
        let removed = self.persist.remove(&lock.conversation.id);
        // Released only once the directories are gone.
        drop(lock);
        Ok(removed?)
    }
}

#[cfg(test)]
#[path = "unlisted_tests.rs"]
mod tests;
