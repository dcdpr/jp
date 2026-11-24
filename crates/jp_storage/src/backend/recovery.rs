//! Conversations a store holds that do not load.
//!
//! A conversation that fails validation never enters the conversation index, so
//! nothing working with loaded conversations can reach it.
//! [`RecoveryBackend`] is how a client finds and acts on one instead: it lists
//! them, describes one by ID, and removes or repairs one once the caller has
//! decided to.

use std::{fmt::Debug, sync::Arc};

use jp_conversation::ConversationId;

use crate::{
    error::Result,
    validate::{InvalidConversation, StorageLocation, ValidationError},
};

/// Access to the conversations a store holds that do not load.
///
/// [`Self::scan`] reads live conversations only, since it runs before every
/// command; the archive can hold thousands of conversations.
/// The other methods reach the archive: [`Self::scan_archive`] and
/// [`Self::diagnose`] read all of it, and a lookup by ID reads it only for a
/// conversation with no live copy.
pub trait RecoveryBackend: Debug + Send + Sync {
    /// Every live conversation that fails validation, sorted by location.
    ///
    /// A conversation another process holds the lock on is left out, because it
    /// may be partway through a write.
    /// The store may remove leftovers of its own interrupted writes, but never
    /// moves or deletes a conversation.
    fn scan(&self) -> Result<Vec<InvalidConversation>>;

    /// Every archived conversation that fails validation, sorted by location,
    /// read without changing anything.
    ///
    /// For a command that lists the archive.
    /// A conversation another process holds the lock on is left out, as in
    /// [`Self::scan`].
    ///
    /// # Errors
    ///
    /// Fails when the store cannot be read.
    fn scan_archive(&self) -> Result<Vec<InvalidConversation>>;

    /// What the store holds for conversation `id`, read without loading it: its
    /// live copy, or its archived copy when it has no live copy.
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

    /// What the store holds that needs the user's attention, live and archived,
    /// read without changing anything.
    ///
    /// A conversation another process holds the lock on is left out, as in
    /// [`Self::scan`].
    ///
    /// # Errors
    ///
    /// Fails when the store cannot be read.
    fn diagnose(&self) -> Result<Diagnosis>;

    /// Apply `repair` to conversation `id` if it makes the conversation load
    /// and `decide` approves of what the store holds for it.
    ///
    /// The conversation's lock is held from reading its state until the repair
    /// is done.
    /// `decide` is not called when another process holds the lock, the store
    /// holds nothing for `id`, or the repair would not make it load.
    ///
    /// # Errors
    ///
    /// Fails when the lock cannot be taken or the repair cannot be written.
    fn repair(
        &self,
        id: &ConversationId,
        repair: Repair,
        decide: &mut dyn FnMut(&StoredConversation) -> bool,
    ) -> Result<RepairOutcome>;
}

/// A change the store can make to a conversation that does not load, so that it
/// does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Repair {
    /// Write the metadata of a conversation missing nothing else, from what the
    /// rest of the conversation holds.
    RebuildMetadata,
}

impl Repair {
    pub const ALL: [Self; 1] = [Self::RebuildMetadata];

    /// A stable name for the repair, by which a user asks for it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::RebuildMetadata => "rebuild-metadata",
        }
    }

    /// The repair called `name`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|repair| repair.name() == name)
    }
}

/// What a store holds that needs the user's attention.
#[derive(Debug, Default)]
pub struct Diagnosis {
    /// Stored conversations that fail validation, sorted by location.
    pub invalid: Vec<DiagnosedConversation>,

    /// What the store holds beside its conversations, sorted by location.
    pub strays: Vec<StrayEntry>,
}

/// Something the store holds beside its conversations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrayEntry {
    /// The name the store lists it under: for the filesystem backend, the
    /// directory in `conversations/` that it is, or that holds it.
    pub name: String,

    pub kind: StrayKind,
    pub location: StorageLocation,
}

/// What a [`StrayEntry`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrayKind {
    /// An entry whose name is not a conversation's.
    NotAConversation,

    /// A file an interrupted write left beside a conversation's parts.
    ///
    /// It holds no user data, and the next [`RecoveryBackend::scan`] removes
    /// it.
    TempFile,

    /// A copy of a conversation an interrupted import left behind.
    ///
    /// The conversation it copies is still stored, and the next
    /// [`RecoveryBackend::scan`] removes it.
    ImportStaging,
}

/// A stored conversation that fails validation, and how it can be made to load.
#[derive(Debug)]
pub struct DiagnosedConversation {
    pub conversation: InvalidConversation,

    /// The repair that makes the conversation load, when one does.
    pub repair: Option<Repair>,
}

/// The outcome of [`RecoveryBackend::repair`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairOutcome {
    Repaired,

    /// `decide` did not approve; the conversation is unchanged.
    Declined,

    /// Another process holds the conversation's lock.
    Locked,

    /// The store holds nothing for the ID.
    NotFound,

    /// The repair would not make the conversation load; it is unchanged.
    NotApplicable,
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

    /// The repair that makes the conversation load, when it does not and one
    /// does.
    pub repair: Option<Repair>,

    /// Whether the conversation is in the archive.
    pub archived: bool,
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

    fn scan_archive(&self) -> Result<Vec<InvalidConversation>> {
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

    fn diagnose(&self) -> Result<Diagnosis> {
        Ok(Diagnosis::default())
    }

    fn repair(
        &self,
        _id: &ConversationId,
        _repair: Repair,
        _decide: &mut dyn FnMut(&StoredConversation) -> bool,
    ) -> Result<RepairOutcome> {
        Ok(RepairOutcome::NotFound)
    }
}

/// Reads through to another backend, and changes nothing.
///
/// [`RecoveryBackend::remove`] and [`RecoveryBackend::repair`] ask `decide` as
/// the backend would and report its answer, without taking a lock or changing
/// the store.
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

    fn scan_archive(&self) -> Result<Vec<InvalidConversation>> {
        self.inner.scan_archive()
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

    fn diagnose(&self) -> Result<Diagnosis> {
        self.inner.diagnose()
    }

    fn repair(
        &self,
        id: &ConversationId,
        repair: Repair,
        decide: &mut dyn FnMut(&StoredConversation) -> bool,
    ) -> Result<RepairOutcome> {
        Ok(match self.inner.inspect(id) {
            None => RepairOutcome::NotFound,
            Some(stored) if stored.repair != Some(repair) => RepairOutcome::NotApplicable,
            Some(stored) if decide(&stored) => RepairOutcome::Repaired,
            Some(_) => RepairOutcome::Declined,
        })
    }
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
