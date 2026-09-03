//! Guard-scoped persistence for conversations.
//!
//! Two types provide guard-scoped persistence:
//!
//! - [`ConversationLock`] — exclusive access to a conversation.
//!   When backed by storage, holds an OS-level `flock` for cross-process
//!   exclusion.
//!   For in-memory workspaces the flock is absent, but the type-level guarantee
//!   (only one lock per conversation) still holds within the process.
//!   Provides read access and produces [`ConversationMut`] scopes for writes.
//!
//! - [`ConversationMut`] — a mutable scope over a conversation.
//!   Automatically persists modified data to disk when dropped (if a persist
//!   backend is configured).
//!   Uses a callback-based API for writes to make it structurally impossible to
//!   hold a write lock guard across `.await` points.
//!
//! # Type Hierarchy
//!
//! ```text
//! ConversationLock
//! ├── Holds Box<dyn ConversationLockGuard>   — cross-process exclusion
//! ├── Holds Arc<RwLock<Conversation>>         — shared with Workspace
//! ├── Holds Arc<RwLock<ConversationStream>>   — shared with Workspace
//! ├── Read methods: metadata(), events()      — return RwLockReadGuard
//! ├── as_mut()   → ConversationMut (borrows lock_guard via Arc clone)
//! └── into_mut() → ConversationMut (consumes lock, takes ownership)
//!
//! ConversationMut
//! ├── Read methods:  metadata(), events()           — return RwLockReadGuard
//! ├── Write methods: update_events(), update_metadata() — callback-based, set dirty
//! ├── flush(&mut self) → explicit persist with error propagation
//! └── Drop: if dirty → read data → persist → lock released when Arc drops
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
//!   in the LLM loop).
//!   I/O errors propagate via `?`, halting the loop.
//! - **`Drop`** — safety net.
//!   If the `ConversationMut` drops while dirty (e.g., due to `?` unwinding),
//!   `Drop` persists the data.
//!
//! Long-running loops should call `flush()` at each checkpoint so disk errors
//! halt immediately rather than letting the loop continue with unsaved data.
//!
//! # Reporting Drop-Time Failures
//!
//! `Drop` cannot propagate, so a failed drop-time persist is recorded on state
//! shared with the originating lock instead of being written to the terminal.
//! The next `flush()` returns it, and
//! [`take_persist_failure`][ConversationLock::take_persist_failure] drains it
//! for a caller that wants to report at teardown.
//! Only the first failure is kept, so one failing disk yields one diagnostic
//! rather than one per mutation scope, and the reporting happens where the
//! output channel and the exit code live.
//!
//! Every later scope still attempts its own write.
//! A persist can span two filesystems (the durable user-local copy and the
//! workspace projection), so a failure against one of them says nothing about
//! the other: skipping subsequent writes would strand new events that the
//! healthy root could still accept.
//!
//! The record is owned by the `Workspace`, not by the lock, so a failure
//! recorded while a dropped future unwinds is still there once the lock is
//! gone.
//! `Workspace::take_persist_failure` is the drain of last resort for a run
//! whose command future was cancelled before it could report.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use jp_conversation::{Conversation, ConversationId, ConversationStream};
use jp_storage::backend::{ConversationLockGuard, PersistBackend, Projection};
use parking_lot::{Mutex, RwLock, RwLockReadGuard};
use tracing::{debug, warn};

use crate::{error::Error, handle::ConversationHandle};

/// Shared record of drop-time persistence outcomes.
///
/// See the module docs for how a recorded failure reaches the user.
#[derive(Debug, Default)]
pub(crate) struct PersistState {
    /// The first drop-time failure no caller has been told about yet.
    ///
    /// The first is kept rather than the last: later failures are consequences
    /// of the same condition, while the first names the write that actually
    /// broke.
    failure: Option<Error>,
}

impl PersistState {
    /// Take the recorded failure, leaving the slot empty.
    pub(crate) fn take(&mut self) -> Option<Error> {
        self.failure.take()
    }
}

/// Shared handle to the record of drop-time persistence failures.
///
/// Held by the workspace and by every lock and scope derived from it, so a
/// failure recorded while a future unwinds outlives the scope that recorded it.
pub(crate) type PersistFailures = Arc<Mutex<PersistState>>;

/// Result of attempting to acquire a conversation lock.
#[derive(Debug)]
pub enum LockResult {
    /// Lock acquired successfully.
    Acquired(ConversationLock),

    /// Another process holds the lock.
    /// The handle is returned so the caller can retry without re-acquiring it.
    AlreadyLocked(ConversationHandle),
}

/// Cross-process exclusive access to a conversation.
///
/// Proves that the `flock` is held.
/// Provides read access and produces [`ConversationMut`] scopes for writes.
///
/// The lock is held for the entire lifetime of this value and released when
/// dropped (or when a `ConversationMut` created via [`into_mut`] drops).
///
/// [`into_mut`]: Self::into_mut
pub struct ConversationLock {
    id: ConversationId,
    metadata: Arc<RwLock<Conversation>>,
    events: Arc<RwLock<ConversationStream>>,
    writer: Arc<dyn PersistBackend>,
    lock_guard: Arc<Box<dyn ConversationLockGuard>>,
    projection: Projection,
    persist: PersistFailures,

    /// Set once this conversation has been written, and shared with the
    /// workspace that handed the lock out.
    ///
    /// Writing happens here and the workspace cannot see it happen, so the
    /// answer to "has this ever been written" has to be left somewhere the
    /// workspace can read later.
    written: Arc<AtomicBool>,
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
        writer: Arc<dyn PersistBackend>,
        lock_guard: Box<dyn ConversationLockGuard>,
        projection: Projection,
        persist: PersistFailures,
        written: Arc<AtomicBool>,
    ) -> Self {
        Self {
            id: handle.into_inner(),
            metadata,
            events,
            writer,
            lock_guard: Arc::new(lock_guard),
            projection,
            persist,
            written,
        }
    }

    /// Take the drop-time persist failure recorded since the last call.
    ///
    /// A caller reports it once, through whatever output channel and exit code
    /// it owns.
    /// Failures already surfaced through [`ConversationMut::flush`] are not
    /// recorded here and are never returned twice.
    #[must_use]
    pub fn take_persist_failure(&self) -> Option<Error> {
        self.persist.lock().take()
    }

    /// The write projection this lock persists with.
    ///
    /// Resolved from storage presence at acquisition (or the creation flags for
    /// a new conversation), and used to decide which roots a write reaches.
    #[must_use]
    pub fn projection(&self) -> Projection {
        self.projection
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

    /// Read the conversation event stream through a callback.
    ///
    /// Read-only counterpart of [`ConversationMut::update_events`]: the shared
    /// guard is scoped to the callback (so it cannot be held across `.await`
    /// points) and nothing is marked dirty, so no persist is triggered.
    /// Use this instead of `as_mut().update_events(..)` whenever the callback
    /// only needs to *read* the stream.
    pub fn with_events<R>(&self, f: impl FnOnce(&ConversationStream) -> R) -> R {
        let guard = self.events.read();
        f(&guard)
    }

    /// Create a short-lived mutable scope.
    /// Persists on drop.
    ///
    /// The lock retains the flock — it outlives the returned
    /// `ConversationMut`.
    /// Use this for multiple mutation phases within a single lock session
    /// (e.g., the turn loop in `jp query`).
    #[must_use]
    pub fn as_mut(&self) -> ConversationMut {
        ConversationMut {
            id: self.id,
            metadata: Arc::clone(&self.metadata),
            events: Arc::clone(&self.events),
            dirty: AtomicBool::new(false),
            writer: Arc::clone(&self.writer),
            projection: self.projection,
            persist: Arc::clone(&self.persist),
            written: Arc::clone(&self.written),
            _lock_guard: Arc::clone(&self.lock_guard),
        }
    }

    /// Consume the lock into a mutable scope that owns the flock.
    ///
    /// The flock is released when the `ConversationMut` drops.
    /// Use this for brief, one-shot mutations (e.g., `conversation edit`,
    /// `config set`).
    #[must_use]
    pub fn into_mut(self) -> ConversationMut {
        ConversationMut {
            id: self.id,
            metadata: self.metadata,
            events: self.events,
            dirty: AtomicBool::new(false),
            writer: self.writer,
            projection: self.projection,
            persist: self.persist,
            written: self.written,
            _lock_guard: self.lock_guard,
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
    writer: Arc<dyn PersistBackend>,
    projection: Projection,

    // Shared with the workspace, the originating lock, and every other scope
    // derived from it, so a failure recorded here survives this scope's drop.
    persist: PersistFailures,

    /// Set once a write succeeds; read by the workspace that handed out the
    /// lock this came from.
    written: Arc<AtomicBool>,

    // Holds the lock guard alive. Released when the last Arc drops.
    _lock_guard: Arc<Box<dyn ConversationLockGuard>>,
}

impl ConversationMut {
    /// The conversation ID this scope covers.
    #[must_use]
    pub fn id(&self) -> ConversationId {
        self.id
    }

    /// Read conversation metadata.
    ///
    /// Returns a `RwLockReadGuard`.
    /// Do **not** hold this across `.await` points — clone the data and drop
    /// the guard first.
    pub fn metadata(&self) -> RwLockReadGuard<'_, Conversation> {
        self.metadata.read()
    }

    /// Read the conversation event stream.
    ///
    /// Returns a `RwLockReadGuard`.
    /// Do **not** hold this across `.await` points — clone the data and drop
    /// the guard first.
    pub fn events(&self) -> RwLockReadGuard<'_, ConversationStream> {
        self.events.read()
    }

    /// Read the conversation event stream through a callback.
    ///
    /// Read-only counterpart of [`Self::update_events`]: the shared guard is
    /// scoped to the callback and the dirty flag is untouched, so no persist is
    /// triggered on drop.
    pub fn with_events<R>(&self, f: impl FnOnce(&ConversationStream) -> R) -> R {
        let guard = self.events.read();
        f(&guard)
    }

    /// Mutate conversation metadata through a callback.
    ///
    /// The write guard is acquired for the duration of the callback and
    /// released when `f` returns.
    /// The dirty flag is set unconditionally.
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
    /// released when `f` returns.
    /// The dirty flag is set unconditionally.
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

    /// Mutate conversation metadata and persist before returning.
    ///
    /// Consumes the scope, so the write has landed by the time this returns: a
    /// caller can report the outcome without the result still being in flight,
    /// and a failed write surfaces as an error rather than a confirmation the
    /// user cannot trust.
    /// The callback's value is forwarded on success.
    ///
    /// For one mutation this replaces `update_metadata` plus an explicit
    /// [`flush`].
    /// When several mutations belong to the same logical change, keep
    /// [`update_metadata`] and flush once at the end: each call here persists
    /// the whole conversation, so a loop of them writes once per iteration.
    ///
    /// [`flush`]: Self::flush
    /// [`update_metadata`]: Self::update_metadata
    pub fn update_metadata_and_flush<R>(
        mut self,
        f: impl FnOnce(&mut Conversation) -> R,
    ) -> crate::error::Result<R> {
        let value = self.update_metadata(f);
        self.flush()?;
        Ok(value)
    }

    /// Mutate the conversation event stream and persist before returning.
    ///
    /// Event-stream counterpart of [`update_metadata_and_flush`]; the same
    /// one-write-per-call caveat applies.
    ///
    /// [`update_metadata_and_flush`]: Self::update_metadata_and_flush
    pub fn update_events_and_flush<R>(
        mut self,
        f: impl FnOnce(&mut ConversationStream) -> R,
    ) -> crate::error::Result<R> {
        let value = self.update_events(f);
        self.flush()?;
        Ok(value)
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
    /// each turn in the LLM loop) so that I/O errors propagate immediately via
    /// `?`.
    ///
    /// Returns a persist failure recorded by an earlier drop-time write before
    /// attempting its own, so a failure that could not propagate from `Drop`
    /// still reaches a caller that can act on it.
    ///
    /// Takes `&mut self` to prevent calling while a write guard from
    /// `update_events()` or `update_metadata()` is held (which would deadlock).
    /// In practice this is already enforced by the callback API, but `&mut
    /// self` makes it explicit.
    ///
    /// After a successful flush, the dirty flag is cleared.
    pub fn flush(&mut self) -> crate::error::Result<()> {
        if let Some(error) = self.persist.lock().take() {
            return Err(error);
        }

        if !self.dirty.load(Ordering::Relaxed) {
            return Ok(());
        }

        let meta = self.metadata.read();
        let evts = self.events.read();
        // Not recorded on the shared state: this error is returned, so the
        // caller owns reporting it. The scope stays dirty, so if the caller
        // swallows it, the drop below retries and records that failure.
        self.writer.write(&self.id, &meta, &evts, self.projection)?;
        self.dirty.store(false, Ordering::Relaxed);

        // After the write, never before: a failed write leaves the conversation
        // as unwritten as it was, and claiming otherwise would have a later
        // index reload treat its absence as somebody else's deletion.
        self.written.store(true, Ordering::Relaxed);

        debug!(id = %self.id, "Flushed conversation to disk.");
        Ok(())
    }

    /// Take the drop-time persist failure recorded since the last call.
    ///
    /// Counterpart of [`ConversationLock::take_persist_failure`] for a scope
    /// that owns the lock, where no `ConversationLock` remains to drain.
    #[must_use]
    pub fn take_persist_failure(&self) -> Option<Error> {
        self.persist.lock().take()
    }

    /// The write projection this scope persists with.
    #[must_use]
    pub fn projection(&self) -> Projection {
        self.projection
    }

    /// Change the write projection and mark the scope dirty.
    ///
    /// The next persist writes to the roots the new projection selects.
    /// Any stale copy in a no-longer-selected root is left in place;
    /// reconciling it is the caller's responsibility.
    pub fn set_projection(&mut self, projection: Projection) {
        self.projection = projection;
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Whether any mutations have occurred since creation or last flush.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    /// Clear the dirty flag without persisting.
    ///
    /// Used by `remove_conversation` to prevent `Drop` from persisting data
    /// that's about to be deleted.
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

        let meta = self.metadata.read();
        let evts = self.events.read();

        match self.writer.write(&self.id, &meta, &evts, self.projection) {
            // Recorded here as well as in `flush`, since most conversations are
            // persisted by going out of scope rather than by an explicit call.
            Ok(()) => self.written.store(true, Ordering::Relaxed),
            Err(error) => {
                let error = Error::from(error);
                warn!(id = %self.id, %error, "Failed to persist conversation.");

                let mut state = self.persist.lock();
                if state.failure.is_none() {
                    state.failure = Some(error);
                }
            }
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
