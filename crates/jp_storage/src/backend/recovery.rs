//! Conversations a store holds that do not load.
//!
//! A conversation that fails validation never enters the conversation index, so
//! nothing working with loaded conversations can reach it.
//! [`RecoveryBackend`] is how a client finds and acts on one instead: it lists
//! them, describes one by ID, and removes one once the caller has decided to.

use std::{fmt::Debug, sync::Arc};

use jp_conversation::ConversationId;

use crate::{
    error::Result,
    validate::{InvalidConversation, StorageLocation, ValidationError},
};

/// Access to the conversations a store holds that do not load.
pub trait RecoveryBackend: Debug + Send + Sync {
    /// Every stored conversation that fails validation, sorted by location.
    ///
    /// A conversation another process holds the lock on is left out, because it
    /// may be partway through a write.
    /// The store may remove leftovers of its own interrupted writes, but never
    /// moves or deletes a conversation.
    fn scan(&self) -> Result<Vec<InvalidConversation>>;

    /// What the store holds for conversation `id`, read without loading it.
    ///
    /// `None` when the store holds nothing for `id`.
    fn inspect(&self, id: &ConversationId) -> Option<StoredConversation>;

    /// Remove conversation `id` if `decide` approves of what the store holds
    /// for it.
    ///
    /// The conversation's lock is held from reading its state until the removal
    /// is done, so `decide` sees exactly what is removed.
    /// `decide` is not called when another process holds the lock or the store
    /// holds nothing for `id`.
    ///
    /// # Errors
    ///
    /// Fails when the lock cannot be taken or the removal fails.
    fn remove(
        &self,
        id: &ConversationId,
        decide: &mut dyn FnMut(&StoredConversation) -> bool,
    ) -> Result<Removal>;
}

/// A conversation's stored parts, as [`RecoveryBackend::inspect`] found them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredConversation {
    pub id: ConversationId,

    /// Where the store holds the conversation; for the filesystem backend, its
    /// directories, workspace first.
    pub locations: Vec<StorageLocation>,

    /// The conversation's parts as validation checks them: the first failure
    /// when any location fails.
    pub state: std::result::Result<(), ValidationError>,

    /// The title stored in the conversation's metadata, when the metadata loads
    /// and has one.
    pub title: Option<String>,
}

/// The outcome of [`RecoveryBackend::remove`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    Removed,

    /// `decide` did not approve; the conversation is unchanged.
    Declined,

    /// Another process holds the conversation's lock.
    Locked,

    /// The store holds nothing for the ID.
    NotFound,
}

/// A store that holds no conversations, such as an in-memory one.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullRecoveryBackend;

impl RecoveryBackend for NullRecoveryBackend {
    fn scan(&self) -> Result<Vec<InvalidConversation>> {
        Ok(vec![])
    }

    fn inspect(&self, _id: &ConversationId) -> Option<StoredConversation> {
        None
    }

    fn remove(
        &self,
        _id: &ConversationId,
        _decide: &mut dyn FnMut(&StoredConversation) -> bool,
    ) -> Result<Removal> {
        Ok(Removal::NotFound)
    }
}

/// Reads through to another backend, and removes nothing.
///
/// [`RecoveryBackend::remove`] asks `decide` as the backend would and reports
/// its answer, without taking a lock or changing the store.
#[derive(Debug, Clone)]
pub struct DryRunRecovery {
    inner: Arc<dyn RecoveryBackend>,
}

impl DryRunRecovery {
    #[must_use]
    pub fn new(inner: Arc<dyn RecoveryBackend>) -> Self {
        Self { inner }
    }
}

impl RecoveryBackend for DryRunRecovery {
    fn scan(&self) -> Result<Vec<InvalidConversation>> {
        self.inner.scan()
    }

    fn inspect(&self, id: &ConversationId) -> Option<StoredConversation> {
        self.inner.inspect(id)
    }

    fn remove(
        &self,
        id: &ConversationId,
        decide: &mut dyn FnMut(&StoredConversation) -> bool,
    ) -> Result<Removal> {
        Ok(match self.inner.inspect(id) {
            None => Removal::NotFound,
            Some(stored) if decide(&stored) => Removal::Removed,
            Some(_) => Removal::Declined,
        })
    }
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
