//! Storage backend trait abstractions.
//!
//! Focused traits decouple conversation persistence, loading, locking, session
//! management, and recovery from the filesystem:
//!
//! - [`PersistBackend`]: writes and removes conversation data.
//! - [`LoadBackend`]: reads conversation data and indexes.
//! - [`LockBackend`]: conversation-level exclusive locking.
//! - [`SessionBackend`]: session-to-conversation mapping storage.
//! - [`RecoveryBackend`]: inspects, diagnoses, repairs, and removes
//!   conversations that fail validation.
//!
//! See also: `docs/rfd/073-layered-storage-backend-for-workspaces.md`

mod fs;
mod load;
mod lock;
mod memory;
mod null;
mod persist;
mod recovery;
mod session;

pub use fs::FsStorageBackend;
pub use load::{ConversationFilter, ConversationIndexEntry, LoadBackend, StoragePresence};
pub use lock::{ConversationLockGuard, LockBackend};
pub use memory::InMemoryStorageBackend;
pub use null::{NoopLockGuard, NullLockBackend, NullPersistBackend, ReadOnlySessionBackend};
pub use persist::{PersistBackend, Projection};
pub use recovery::{
    DiagnosedConversation, Diagnosis, DryRunRecovery, NullRecoveryBackend, RecoveryBackend,
    Removal, Repair, RepairOutcome, StoredConversation, StrayEntry, StrayKind,
};
pub use session::SessionBackend;
