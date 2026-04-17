//! Storage backend trait abstractions.
//!
//! Four focused traits decouple conversation persistence, loading, locking, and
//! session management from the filesystem:
//!
//! - [`PersistBackend`] — writes and removes conversation data.
//! - [`LoadBackend`] — reads conversation data and indexes.
//! - [`LockBackend`] — conversation-level exclusive locking.
//! - [`SessionBackend`] — session-to-conversation mapping storage.
//!
//! See also: `docs/rfd/073-layered-storage-backend-for-workspaces.md`

mod fs;
mod load;
mod lock;
mod memory;
mod null;
mod persist;
mod session;

pub use fs::FsStorageBackend;
pub use load::{ConversationFilter, LoadBackend, SanitizeReport, TrashedConversation};
pub use lock::{ConversationLockGuard, LockBackend};
pub use memory::InMemoryStorageBackend;
pub use null::{NoopLockGuard, NullLockBackend, NullPersistBackend};
pub use persist::PersistBackend;
pub use session::SessionBackend;
