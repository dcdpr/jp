//! Guard-scoped persistence for conversations (RFD D02).
//!
//! Two types provide guard-scoped persistence:
//!
//! - [`ConversationLock`] — cross-process exclusive access to a conversation.
//!   Holds the OS-level `flock` and provides read access. Produces
//!   [`ConversationMut`] scopes for writes.
//!
//! - [`ConversationMut`] — a mutable scope over a conversation. Automatically
//!   persists modified data to disk when dropped. Uses a callback-based API
//!   for writes to make it structurally impossible to hold a write lock guard
//!   across `.await` points.
//!
//! # Type Hierarchy
//!
//! ```text
//! ConversationLock
//! ├── Holds Arc<ConversationFileLock>       — cross-process exclusion
//! ├── Holds Arc<RwLock<Conversation>>       — shared with Workspace
//! ├── Holds Arc<RwLock<ConversationStream>> — shared with Workspace
//! ├── Read methods: metadata(), events()    — return RwLockReadGuard
//! ├── as_mut()   → ConversationMut (borrows flock via Arc clone)
//! └── into_mut() → ConversationMut (consumes lock, takes flock ownership)
//!
//! ConversationMut
//! ├── Read methods:  metadata(), events()           — return RwLockReadGuard
//! ├── Write methods: update_events(), update_metadata() — callback-based, set dirty
//! ├── flush(&mut self) → explicit persist with error propagation
//! └── Drop: if dirty → read data → persist → flock released when last Arc drops
//! ```
//!
//! # Callback-Based Mutation
//!
//! Write access uses callbacks instead of returning raw `RwLockWriteGuard`s.
//! This makes it structurally impossible to hold a write lock across `.await`
//! points — the guard's scope is bounded by the closure:
//!
//! ```ignore
//! // The write guard exists only inside the closure.
//! conv.update_events(|events| {
//!     events.current_turn_mut().add_tool_response(resp);
//! });
//!
//! // Error propagation composes naturally:
//! conv.update_events(|events| {
//!     turn_coordinator.start_turn(events, request.clone());
//!     this_can_error()?;
//!     Ok(())
//! })?;
//! ```
//!
//! # Persistence Model
//!
//! - **`flush()?`** — explicit persist at checkpoints (e.g., after each turn
//!   in the LLM loop). I/O errors propagate via `?`, halting the loop.
//! - **`Drop`** — safety net. If the `ConversationMut` drops while dirty
//!   (e.g., due to `?` unwinding), `Drop` persists the data. Errors are
//!   logged but cannot be propagated from `Drop`.
//!
//! Long-running loops should call `flush()` at each checkpoint so disk errors
//! halt immediately rather than letting the loop continue with unsaved data.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use jp_conversation::{Conversation, ConversationId, ConversationStream};
use jp_storage::lock::ConversationFileLock;
use parking_lot::{RwLock, RwLockReadGuard};
use tracing::info;

use crate::{handle::ConversationHandle, persist::PersistBackend};

/// Cross-process exclusive access to a conversation.
///
/// Proves that the `flock` is held. Provides read access and produces
/// [`ConversationMut`] scopes for writes.
///
/// The lock is held for the entire lifetime of this value and released when
/// dropped (or when a `ConversationMut` created via [`into_mut`] drops).
///
/// [`into_mut`]: Self::into_mut
pub struct ConversationLock {
    id: ConversationId,
    metadata: Arc<RwLock<Conversation>>,
    events: Arc<RwLock<ConversationStream>>,
    writer: Option<Arc<dyn PersistBackend>>,
    file_lock: Arc<ConversationFileLock>,
}

impl ConversationLock {
    /// Create a new `ConversationLock`, consuming the handle.
    ///
    /// The handle is proof that the conversation exists in the workspace index.
    /// Consuming it here enforces that only one access token (either a handle
    /// or a lock) exists per conversation at a time.
    pub(crate) fn new(
        handle: ConversationHandle,
        metadata: Arc<RwLock<Conversation>>,
        events: Arc<RwLock<ConversationStream>>,
        writer: Option<Arc<dyn PersistBackend>>,
        file_lock: ConversationFileLock,
    ) -> Self {
        Self {
            id: handle.into_inner(),
            metadata,
            events,
            writer,
            file_lock: Arc::new(file_lock),
        }
    }

    /// The conversation ID this lock protects.
    #[must_use]
    pub fn id(&self) -> ConversationId {
        self.id
    }

    /// Read conversation metadata.
    pub fn metadata(&self) -> RwLockReadGuard<'_, Conversation> {
        self.metadata.read()
    }

    /// Read the conversation event stream.
    pub fn events(&self) -> RwLockReadGuard<'_, ConversationStream> {
        self.events.read()
    }

    /// Create a short-lived mutable scope. Persists on drop.
    ///
    /// The lock retains the flock — it outlives the returned `ConversationMut`.
    /// Use this for multiple mutation phases within a single lock session
    /// (e.g., the turn loop in `jp query`).
    #[must_use]
    pub fn as_mut(&self) -> ConversationMut {
        ConversationMut {
            id: self.id,
            metadata: Arc::clone(&self.metadata),
            events: Arc::clone(&self.events),
            dirty: AtomicBool::new(false),
            writer: self.writer.clone(),
            _file_lock: Arc::clone(&self.file_lock),
        }
    }

    /// Consume the lock into a mutable scope that owns the flock.
    ///
    /// The flock is released when the `ConversationMut` drops. Use this for
    /// brief, one-shot mutations (e.g., `conversation edit`, `config set`).
    #[must_use]
    pub fn into_mut(self) -> ConversationMut {
        ConversationMut {
            id: self.id,
            metadata: self.metadata,
            events: self.events,
            dirty: AtomicBool::new(false),
            writer: self.writer,
            _file_lock: self.file_lock,
        }
    }

    /// Create a test lock with no-op flock and no persistence.
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    #[must_use]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "handle is consumed intentionally"
    )]
    pub fn test_lock(
        handle: ConversationHandle,
        metadata: Arc<RwLock<Conversation>>,
        events: Arc<RwLock<ConversationStream>>,
    ) -> Self {
        Self {
            id: handle.id(),
            metadata,
            events,
            writer: None,
            file_lock: Arc::new(ConversationFileLock::test_noop()),
        }
    }

    /// Create a test lock with a mock persist backend.
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    #[must_use]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "handle is consumed intentionally"
    )]
    pub fn test_lock_with_writer(
        handle: ConversationHandle,
        metadata: Arc<RwLock<Conversation>>,
        events: Arc<RwLock<ConversationStream>>,
        writer: Arc<dyn PersistBackend>,
    ) -> Self {
        Self {
            id: handle.id(),
            metadata,
            events,
            writer: Some(writer),
            file_lock: Arc::new(ConversationFileLock::test_noop()),
        }
    }
}

impl std::fmt::Debug for ConversationLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConversationLock")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// A mutable scope over a conversation with automatic persistence on drop.
///
/// Created from a [`ConversationLock`] via [`as_mut()`] or [`into_mut()`].
///
/// Write access uses callbacks (`update_events`, `update_metadata`) to make it
/// structurally impossible to hold a `RwLockWriteGuard` across `.await` points.
///
/// When dropped, if any mutation occurred (the dirty flag is set), the
/// conversation data is persisted to disk while the flock is still held.
///
/// [`as_mut()`]: ConversationLock::as_mut
/// [`into_mut()`]: ConversationLock::into_mut
pub struct ConversationMut {
    id: ConversationId,
    metadata: Arc<RwLock<Conversation>>,
    events: Arc<RwLock<ConversationStream>>,
    dirty: AtomicBool,
    writer: Option<Arc<dyn PersistBackend>>,
    // Holds the flock alive. Released when the last Arc reference drops.
    _file_lock: Arc<ConversationFileLock>,
}

impl ConversationMut {
    /// The conversation ID this scope covers.
    #[must_use]
    pub fn id(&self) -> ConversationId {
        self.id
    }

    /// Read conversation metadata.
    ///
    /// Returns a `RwLockReadGuard`. Do **not** hold this across `.await`
    /// points — clone the data and drop the guard first.
    pub fn metadata(&self) -> RwLockReadGuard<'_, Conversation> {
        self.metadata.read()
    }

    /// Read the conversation event stream.
    ///
    /// Returns a `RwLockReadGuard`. Do **not** hold this across `.await`
    /// points — clone the data and drop the guard first.
    pub fn events(&self) -> RwLockReadGuard<'_, ConversationStream> {
        self.events.read()
    }

    /// Mutate conversation metadata through a callback.
    ///
    /// The write guard is acquired for the duration of the callback and
    /// released when `f` returns. The dirty flag is set unconditionally.
    ///
    /// The callback's return value is forwarded, so `?` composes naturally:
    ///
    /// ```ignore
    /// conv.update_metadata(|meta| {
    ///     meta.title = Some(new_title);
    /// });
    ///
    /// conv.update_metadata(|meta| -> Result<()> {
    ///     validate(meta)?;
    ///     Ok(())
    /// })?;
    /// ```
    pub fn update_metadata<R>(&self, f: impl FnOnce(&mut Conversation) -> R) -> R {
        self.dirty.store(true, Ordering::Relaxed);
        let mut guard = self.metadata.write();
        f(&mut guard)
    }

    /// Mutate the conversation event stream through a callback.
    ///
    /// The write guard is acquired for the duration of the callback and
    /// released when `f` returns. The dirty flag is set unconditionally.
    ///
    /// ```ignore
    /// conv.update_events(|events| {
    ///     events.add_config_delta(delta);
    /// });
    /// ```
    pub fn update_events<R>(&self, f: impl FnOnce(&mut ConversationStream) -> R) -> R {
        self.dirty.store(true, Ordering::Relaxed);
        let mut guard = self.events.write();
        f(&mut guard)
    }

    /// Mutate both metadata and events atomically through a callback.
    ///
    /// Both write guards are acquired for the duration of the callback.
    /// Useful when a mutation touches both (e.g., creating a conversation).
    pub fn update<R>(&self, f: impl FnOnce(&mut Conversation, &mut ConversationStream) -> R) -> R {
        self.dirty.store(true, Ordering::Relaxed);
        let mut meta = self.metadata.write();
        let mut events = self.events.write();
        f(&mut meta, &mut events)
    }

    /// Persist the current state to disk immediately.
    ///
    /// Long-running loops **must** call this at each checkpoint (e.g., after
    /// each turn in the LLM loop) so that I/O errors propagate immediately
    /// via `?`. The `Drop` implementation is a safety net for `?` unwinding —
    /// it persists partial state but swallows errors.
    ///
    /// Takes `&mut self` to prevent calling while a write guard from
    /// `update_events()` or `update_metadata()` is held (which would
    /// deadlock). In practice this is already enforced by the callback API,
    /// but `&mut self` makes it explicit.
    ///
    /// After a successful flush, the dirty flag is cleared.
    pub fn flush(&mut self) -> crate::error::Result<()> {
        if !self.dirty.load(Ordering::Relaxed) {
            return Ok(());
        }

        let Some(writer) = &self.writer else {
            return Ok(());
        };

        let meta = self.metadata.read();
        let evts = self.events.read();
        writer.write(&self.id, &meta, &evts)?;
        self.dirty.store(false, Ordering::Relaxed);

        info!(id = %self.id, "Flushed conversation to disk.");
        Ok(())
    }

    /// Whether any mutations have occurred since creation or last flush.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    /// Clear the dirty flag without persisting.
    ///
    /// Used by `remove_conversation` to prevent `Drop` from persisting
    /// data that's about to be deleted.
    pub(crate) fn clear_dirty(&self) {
        self.dirty.store(false, Ordering::Relaxed);
    }
}

// Static assertion: ConversationMut must be Send + Sync so it can be
// held across .await points in async code. It only holds Arc, AtomicBool,
// and ConversationId — no lock guards.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ConversationMut>();
};

impl Drop for ConversationMut {
    fn drop(&mut self) {
        if !self.dirty.load(Ordering::Relaxed) {
            return;
        }

        let Some(writer) = &self.writer else {
            return;
        };

        let meta = self.metadata.read();
        let evts = self.events.read();

        #[expect(clippy::print_stderr)]
        if let Err(e) = writer.write(&self.id, &meta, &evts) {
            eprintln!("Failed to persist conversation {}: {e}", self.id);
        }
    }
}

impl std::fmt::Debug for ConversationMut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConversationMut")
            .field("id", &self.id)
            .field("dirty", &self.dirty.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[path = "conversation_lock_tests.rs"]
mod tests;
