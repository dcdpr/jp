//! Type-level access control for conversations.
//!
//! [`ConversationHandle`] is a move-only token proving that a conversation ID
//! exists in the workspace index. It is obtained through
//! [`Workspace::acquire_conversation`] and consumed by
//! [`Workspace::lock_conversation`] to produce a [`ConversationLock`].
//!
//! [`Workspace::acquire_conversation`]: super::Workspace::acquire_conversation
//! [`Workspace::lock_conversation`]: super::Workspace::lock_conversation
//! [`ConversationLock`]: super::ConversationLock

use jp_conversation::ConversationId;

/// Proof that a conversation exists in the workspace index.
#[derive(Debug, PartialEq, Eq)]
pub struct ConversationHandle {
    id: ConversationId,
}

impl ConversationHandle {
    pub(crate) fn new(id: ConversationId) -> Self {
        Self { id }
    }

    /// The conversation ID this handle refers to.
    #[must_use]
    pub fn id(&self) -> ConversationId {
        self.id
    }

    /// Consume this handle and return the underlying conversation ID.
    #[must_use]
    pub fn into_inner(self) -> ConversationId {
        self.id
    }
}
