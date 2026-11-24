//! Inspection, diagnosis, repair, and removal of conversations that fail
//! validation.
//!
//! A conversation that fails validation is excluded from the conversation
//! index, so normal loading cannot reach it.
//! [`RecoveryBackend`] provides direct access to those conversations: listing
//! failures, inspecting a conversation by ID, and repairing or removing it
//! after caller confirmation.

use std::{fmt::Debug, sync::Arc};

use jp_conversation::ConversationId;

use crate::{
    error::Result,
    validate::{InvalidConversation, StorageLocation, ValidationError},
};

/// Access to stored conversations that fail validation.
///
/// [`Self::scan`] checks live conversations only, because it runs before every
/// command and the archive can hold thousands of conversations.
/// [`Self::scan_archive`] and [`Self::diagnose`] also check the archive, and
/// ID-based methods fall back to the archive when a conversation has no live
/// copy.
pub trait RecoveryBackend: Debug + Send + Sync {
    /// Return every live conversation that fails validation, sorted by
    /// location.
    ///
    /// Conversations locked by another process are skipped, because they may be
    /// mid-write.
    /// The scan may clean up temporary files from interrupted writes, but never
    /// moves or deletes a conversation.
    fn scan(&self) -> Result<Vec<InvalidConversation>>;

    /// Return every archived conversation that fails validation, sorted by
    /// location, without modifying the store.
    ///
    /// Conversations locked by another process are skipped.
    ///
    /// # Errors
    ///
    /// Returns an error if the store cannot be read.
    fn scan_archive(&self) -> Result<Vec<InvalidConversation>>;

    /// Inspect the stored state of conversation `id` without loading it.
    ///
    /// Returns the live copy if one exists, or the archived copy otherwise.
    /// Returns `None` if the store has no entry for `id`.
    fn inspect(&self, id: &ConversationId) -> Option<StoredConversation>;

    /// Remove conversation `id` if `decide` returns `true` for its stored
    /// state.
    ///
    /// Holds the conversation lock from inspection through deletion so `decide`
    /// sees the exact state being removed.
    /// `decide` is not called when another process holds the lock or the store
    /// has no entry for `id`.
    ///
    /// # Errors
    ///
    /// Returns an error if acquiring the lock or deleting the conversation
    /// fails.
    fn remove(
        &self,
        id: &ConversationId,
        decide: &mut dyn FnMut(&StoredConversation) -> bool,
    ) -> Result<Removal>;

    /// Diagnose live and archived conversations and stray entries without
    /// modifying the store.
    ///
    /// Conversations locked by another process are skipped.
    ///
    /// # Errors
    ///
    /// Returns an error if the store cannot be read.
    fn diagnose(&self) -> Result<Diagnosis>;

    /// Apply `repair` to conversation `id` if the repair is applicable and
    /// `decide` returns `true`.
    ///
    /// Holds the conversation lock from inspection through repair.
    /// `decide` is not called when another process holds the lock, the store
    /// has no entry for `id`, or `repair` does not apply to the conversation.
    ///
    /// # Errors
    ///
    /// Returns an error if acquiring the lock or writing the repair fails.
    fn repair(
        &self,
        id: &ConversationId,
        repair: Repair,
        decide: &mut dyn FnMut(&StoredConversation) -> bool,
    ) -> Result<RepairOutcome>;
}

/// A repair operation that can make an invalid conversation loadable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Repair {
    /// Rebuild `metadata.json` for a conversation whose other files are valid.
    RebuildMetadata,
}

impl Repair {
    pub const ALL: [Self; 1] = [Self::RebuildMetadata];

    /// CLI identifier for this repair.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::RebuildMetadata => "rebuild-metadata",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|repair| repair.name() == name)
    }
}

/// Problems found in a conversation store by [`RecoveryBackend::diagnose`].
#[derive(Debug, Default)]
pub struct Diagnosis {
    /// Stored conversations that fail validation, sorted by location.
    pub invalid: Vec<DiagnosedConversation>,

    /// Non-conversation entries and leftover temporary files in the store,
    /// sorted by location.
    pub strays: Vec<StrayEntry>,
}

/// A non-conversation entry or leftover temporary file in the conversation
/// store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrayEntry {
    /// Entry name under `conversations/`, or the parent conversation directory
    /// name for a leftover temporary file.
    pub name: String,

    pub kind: StrayKind,
    pub location: StorageLocation,
}

/// Classification of a [`StrayEntry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrayKind {
    /// A directory under `conversations/` whose name is not a conversation ID.
    NotAConversation,

    /// An orphaned `.tmp` file left in a conversation directory by an
    /// interrupted write.
    ///
    /// Holds no user data; removed automatically by the next
    /// [`RecoveryBackend::scan`].
    TempFile,

    /// A staging directory left behind by an interrupted conversation import.
    ///
    /// The source conversation remains intact; removed automatically by the
    /// next [`RecoveryBackend::scan`].
    ImportStaging,
}

/// An invalid conversation paired with the repair that can fix it, if any.
#[derive(Debug)]
pub struct DiagnosedConversation {
    pub conversation: InvalidConversation,
    pub repair: Option<Repair>,

    /// `true` for an archived copy of a conversation that also has a live copy.
    ///
    /// [`RecoveryBackend::inspect`], [`RecoveryBackend::remove`], and
    /// [`RecoveryBackend::repair`] resolve the ID to the live copy, so they
    /// cannot act on this one.
    pub shadowed: bool,
}

/// Outcome of [`RecoveryBackend::repair`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairOutcome {
    Repaired,

    /// `decide` returned `false`; the conversation was not modified.
    Declined,

    /// Another process holds the conversation lock.
    Locked,

    /// No conversation with this ID exists in the store.
    NotFound,

    /// The requested repair cannot fix this conversation.
    NotApplicable,
}

/// On-disk state of a conversation inspected via [`RecoveryBackend::inspect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredConversation {
    pub id: ConversationId,

    /// Storage locations holding the conversation, ordered with workspace
    /// storage before user-local storage.
    pub locations: Vec<StorageLocation>,

    /// Validation result across all locations, or the first validation error
    /// encountered.
    pub state: std::result::Result<(), ValidationError>,

    /// Title from `metadata.json`, if the metadata is readable and has a title.
    pub title: Option<String>,

    /// Repair that can make the conversation loadable, if any.
    pub repair: Option<Repair>,

    pub archived: bool,
}

/// Outcome of [`RecoveryBackend::remove`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    Removed,

    /// `decide` returned `false`; the conversation was not removed.
    Declined,

    /// Another process holds the conversation lock.
    Locked,

    /// No conversation with this ID exists in the store.
    NotFound,
}

/// No-op [`RecoveryBackend`] for stores without persistent recovery (such as
/// in-memory backends).
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

/// Read-only [`RecoveryBackend`] wrapper for `--no-persist` runs.
///
/// [`RecoveryBackend::remove`] and [`RecoveryBackend::repair`] invoke `decide`
/// against the inspected state and return the resulting outcome without
/// acquiring locks or modifying the store.
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
