# RFD 069: Guard-Scoped Persistence for Conversations

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-26
- **Extends**: [RFD 020](020-parallel-conversations.md)

## Summary

Conversation data is automatically persisted to disk when a `ConversationMut`
drops, while the cross-process file lock is still held. The workspace API
produces `ConversationLock`s (cross-process exclusive access) from which
`ConversationMut`s (mutable scopes with auto-persist) are derived. No manual
persist calls are needed. The standard `?` operator works freely — early returns
trigger the `ConversationMut`'s `Drop`, which persists and then releases or
retains the lock depending on ownership.

## Motivation

Persistence of conversation data needs two guarantees: the data reaches disk
while the cross-process lock is held (no race window), and every mutation path
persists without requiring manual calls (no silent data loss). An explicit
`persist_conversation()` API satisfies the first but not the second — missing a
call is a silent bug. A `Workspace::Drop` safety net satisfies the second but
not the first — by the time `Workspace` drops, the lock has been released.

This RFD solves both by moving persistence into `ConversationMut`'s `Drop`. The
`ConversationMut` has everything it needs: shared references to the conversation
data via `Arc<RwLock<...>>`, a write handle to storage via `PersistBackend`, and
shared ownership of the file lock via `Arc`. When the mutable scope ends, data
is written to disk while the lock is held. Callers mutate freely and never think
about persistence.

## Design

### Per-Conversation Interior Mutability

Conversation data in the workspace state is wrapped in `Arc<RwLock<...>>` at
the individual conversation level:

```rust
pub(super) struct State {
    pub(super) conversations: HashMap<ConversationId, OnceLock<Arc<RwLock<Conversation>>>>,
    pub(super) events: HashMap<ConversationId, OnceLock<Arc<RwLock<ConversationStream>>>>,
}
```

`OnceLock` provides lazy initialization (loaded from disk on first access).
`Arc` enables shared ownership between the workspace and any active locks or
mutable scopes. `RwLock` (`parking_lot::RwLock`) allows concurrent reads and
exclusive writes within the process.

Wrapping individual conversations rather than the entire `State` means:

- Locking is per-conversation, not global. Accessing conversation A does not
  block access to conversation B.
- No `MappedMutexGuard` is needed — methods call `.read()` or `.write()`
  directly on the conversation's `RwLock`.
- `HashMap` handles lookup. No change tracking (`TombMap`) is needed because
  `ConversationMut`'s `dirty` flag and auto-persist-on-drop replace all
  modification tracking, and `remove_conversation_with_lock` handles directory
  deletion immediately.

The `Arc<RwLock<...>>` is never exposed outside the `jp_workspace` crate. All
public APIs return lock guards or use callbacks, preserving the invariant that
mutation requires holding the cross-process `flock`.

### Type Hierarchy

```
ConversationLock
├── Holds Arc<ConversationFileLock>       — cross-process exclusion
├── Holds Arc<RwLock<Conversation>>       — shared with Workspace
├── Holds Arc<RwLock<ConversationStream>> — shared with Workspace
├── Read methods: metadata(), events()    — return RwLockReadGuard
├── as_mut()   → ConversationMut (borrows flock via Arc clone)
└── into_mut() → ConversationMut (consumes lock, takes flock ownership)

ConversationMut
├── Holds Arc<RwLock<Conversation>>       — shared with Lock + Workspace
├── Holds Arc<RwLock<ConversationStream>> — shared with Lock + Workspace
├── Holds Arc<ConversationFileLock>       — shared with Lock (or sole owner)
├── Holds Arc<dyn PersistBackend>         — disk write capability
├── Holds AtomicBool                      — dirty flag
├── Read methods:  metadata(), events()           — return RwLockReadGuard
├── Write methods: update_events(), update_metadata() — callback-based, set dirty
├── flush(&mut self)  → explicit persist with error propagation
└── Drop: if dirty → read data → persist → flock released when last Arc drops
```

### `ConversationLock`

Cross-process exclusive access to a conversation. Proves that the `flock` is
held. Provides read access and produces `ConversationMut` scopes for writes.

```rust
pub struct ConversationLock {
    id: ConversationId,
    metadata: Arc<RwLock<Conversation>>,
    events: Arc<RwLock<ConversationStream>>,
    writer: Option<Arc<dyn PersistBackend>>,
    file_lock: Arc<ConversationFileLock>,
}

impl ConversationLock {
    pub fn id(&self) -> ConversationId;

    pub fn metadata(&self) -> RwLockReadGuard<'_, Conversation>;
    pub fn events(&self) -> RwLockReadGuard<'_, ConversationStream>;

    /// Create a short-lived mutable scope. Persists on drop.
    /// The lock retains the flock — it outlives the ConversationMut.
    pub fn as_mut(&self) -> ConversationMut;

    /// Consume the lock into a mutable scope that owns the flock.
    /// The flock is released when the ConversationMut drops.
    pub fn into_mut(self) -> ConversationMut;
}
```

### `ConversationMut`

A mutable scope over a conversation. Automatically persists modified data to
disk when dropped. `ConversationMut` is `Send + Sync` — it holds `Arc`s and
`AtomicBool`, no lock guards — so it can safely be held across `.await` points.

#### Callback-Based Mutation

Write access uses callbacks instead of returning raw `RwLockWriteGuard`s.
This makes it structurally impossible to hold a write lock across `.await`
points — the guard's scope is bounded by the closure:

```rust
impl ConversationMut {
    pub fn update_metadata<R>(&self, f: impl FnOnce(&mut Conversation) -> R) -> R;
    pub fn update_events<R>(&self, f: impl FnOnce(&mut ConversationStream) -> R) -> R;
    pub fn update<R>(&self, f: impl FnOnce(&mut Conversation, &mut ConversationStream) -> R) -> R;
}
```

The write guard is acquired for the duration of the callback and released when
`f` returns. The dirty flag is set unconditionally. The callback's return value
is forwarded, so `?` composes naturally:

```rust
conv.update_events(|events| {
    turn_coordinator.start_turn(events, request.clone());
    this_can_error()?;
    Ok(())
})?;
```

#### Persistence Model

```rust
impl ConversationMut {
    /// Persist the current state to disk immediately.
    ///
    /// Long-running loops must call this at each checkpoint so I/O
    /// errors propagate via `?`. Drop is the safety net for unwinding.
    ///
    /// Takes `&mut self` to prevent calling while a write guard from
    /// update_events() is held (which would deadlock).
    pub fn flush(&mut self) -> Result<()>;
}

impl Drop for ConversationMut {
    fn drop(&mut self) {
        if !self.dirty.load(Ordering::Relaxed) { return; }
        if let Some(writer) = &self.writer {
            let meta = self.metadata.read();
            let evts = self.events.read();
            if let Err(e) = writer.write(&self.id, &meta, &evts) {
                eprintln!("Failed to persist conversation {}: {e}", self.id);
            }
        }
    }
}
```

`AtomicBool` is used for the dirty flag instead of `Cell<bool>`.
`Cell<bool>` is `!Sync`, which would make `ConversationMut` `!Sync` and cause
async futures holding `&ConversationMut` across `.await` points to become
`!Send`. `AtomicBool` with `Ordering::Relaxed` provides the same interior
mutability without the `!Sync` constraint.

### `PersistBackend` Trait

Persistence is abstracted behind a trait so tests can assert persist behavior
without disk I/O:

```rust
pub trait PersistBackend: Send + Sync + Debug {
    fn write(&self, id: &ConversationId, metadata: &Conversation,
             events: &ConversationStream) -> Result<()>;
    fn remove(&self, id: &ConversationId) -> Result<()>;
}
```

The production implementation (`FsPersistBackend`) extracts the write paths
from `Storage` at construction time so persistence can be invoked from
`ConversationMut::Drop` without requiring a reference to `Storage`.

Tests use a `MockPersistBackend` that records calls, or `None` to skip
persistence entirely.

### Workspace API

```rust
impl Workspace {
    /// Acquire an exclusive cross-process lock on a conversation.
    pub fn lock_conversation(&self, handle: ConversationHandle,
                             session: Option<&str>) -> Result<Option<ConversationLock>>;

    /// Read access via handle. Returns a read guard.
    pub fn metadata(&self, h: &ConversationHandle) -> RwLockReadGuard<'_, Conversation>;
    pub fn events(&self, h: &ConversationHandle) -> RwLockReadGuard<'_, ConversationStream>;

    /// Iterate all conversations. Each item yields a read guard.
    pub fn conversations(&self)
        -> impl Iterator<Item = (&ConversationId, ArcRwLockReadGuard<RawRwLock, Conversation>)>;

    /// Remove a conversation, consuming its lock.
    pub fn remove_conversation_with_lock(&mut self, conv: ConversationMut) -> Option<Conversation>;

    pub fn acquire_conversation(&self, id: &ConversationId) -> Result<ConversationHandle>;
    pub fn create_conversation(&mut self, ...) -> ConversationId;
}
```

Read methods on `Workspace` return `RwLockReadGuard` (handle-based) or
`ArcRwLockReadGuard` (iterator). These auto-deref to `&T`. Callers cannot
call `.write()` through them — mutation requires a `ConversationLock`.

`lock_conversation` takes `&self` because it only interacts with the `Storage`
layer for flock acquisition and clones `Arc`s from the state. No data is moved
out of the workspace.

`remove_conversation_with_lock` consumes a `ConversationMut` by value. It
clears the dirty flag to prevent `Drop` from persisting data that's about to
be deleted, then deletes the conversation's directory immediately via the
`PersistBackend`. The conversation is removed from the `HashMap` index.

### Usage Patterns

#### Brief lock (`conversation edit`, `config set`, `conversation fork`)

```rust
let conv = workspace.lock_conversation(handle, session)?
    .ok_or(Error::LockTimeout(id))?
    .into_mut();

conv.update_metadata(|m| m.title = Some(title));
// conv drops -> persist -> flock released
```

#### Session lock (`jp query`)

```rust
let lock = workspace.lock_conversation(handle, session)?
    .ok_or(Error::LockTimeout(id))?;

let title = lock.metadata().title.clone();

lock.as_mut().update_events(|e| e.add_config_delta(delta));

run_turn_loop(..., &lock, ...).await;

let events = lock.events().clone();
drop(lock); // flock released
```

#### Turn loop

The turn loop takes `&ConversationLock` and creates `ConversationMut` scopes
as needed. `ConversationMut` is held across `.await` points safely (it's
`Send + Sync`). Write guards from `update_events` are scoped to the callback
and never cross yield points.

```rust
async fn run_turn_loop(..., lock: &ConversationLock, ...) {
    loop {
        lock.as_mut().update_events(|stream| {
            turn_coordinator.start_turn(stream, request.clone());
        });

        let mut conv = lock.as_mut();
        while let Some(event) = streams.next().await {
            conv.update_events(|stream| {
                handle_llm_event(event, &mut turn_coordinator, stream)
            });
        }
        conv.flush()?;
    }
}
```

#### Async functions needing mutable stream access

Functions that need `&mut ConversationStream` across `.await` points take
`&ConversationMut` and acquire the write lock per-access via callbacks:

```rust
async fn execute_with_prompting(&mut self, conv: &ConversationMut, ...) {
    conv.update_events(|e| e.current_turn_mut().add_inquiry_response(resp));
    some_async_call().await;
    conv.update_events(|e| e.current_turn_mut().add_tool_response(resp));
}
```

### Data Visibility

Because `Arc<RwLock<...>>` is shared between the workspace and the lock/mut,
readers can access conversation data at any time. No data is "checked out" or
hidden. The workspace always has the data. Write locks are held only for
individual callback invocations, so contention is negligible.

### Test Support

`Workspace::test_lock` creates a lock backed by a no-op flock. If the
workspace has storage configured, the test lock automatically attaches the
real `FsPersistBackend` — tests that assert on-disk persistence work without
extra setup. In-memory-only workspaces produce locks with `writer: None`,
skipping persistence entirely.

## Drawbacks

**Read-path API change.** `workspace.metadata(&handle)` returns
`RwLockReadGuard<Conversation>` instead of `&Conversation`. Auto-deref makes
most call sites transparent, but explicit type annotations need adjustment,
and guards must not be held across `.await` points (clone-and-drop instead).

**Callback ergonomics.** Write access uses `conv.update_events(|e| ...)`
instead of `conv.events_mut().do_thing()`. This is slightly more verbose but
structurally prevents `.await`-across-lock-guard bugs. `?` composes naturally
since the callback's return type is forwarded.

**Errors in `Drop` are swallowed.** If persist fails during
`ConversationMut`'s drop, the error is logged to stderr but cannot be
propagated. Long-running loops must call `flush()?` at checkpoints so that
I/O failures halt immediately.

**`parking_lot` dependency.** `parking_lot::RwLock` is used instead of
`std::sync::RwLock` for non-poisoning locks, `DerefMut` on write guards, and
`ArcRwLockReadGuard` for `'static` lifetime guards. `parking_lot` is already
a transitive dependency.

**`Arc` overhead.** Each conversation wraps metadata and events in
`Arc<RwLock<...>>`. The `Arc` adds a heap allocation and reference count per
conversation — negligible for the typical workspace with tens to hundreds of
conversations.

## Alternatives

### Raw write guard access (no callbacks)

Expose `events_mut()` and `metadata_mut()` returning `RwLockWriteGuard`
directly. More ergonomic for synchronous code, but `RwLockReadGuard` (which
is `Send`) can be held across `.await` without compiler errors, deadlocking
silently at runtime. The callback approach makes this structurally impossible.

### Guard owns data (checkout model)

The guard takes ownership of conversation data. The workspace state is empty
while the guard is alive. Achieves auto-persist without `Arc<RwLock>`, but
hides the data from `workspace.conversations()` during the guard's lifetime.

### Whole-state locking (`Arc<Mutex<State>>`)

A single lock for the entire state. Requires `MappedMutexGuard` to navigate
to a specific conversation and means any access to any conversation locks
everything.

### Mandatory-resolution guard (panic-in-Drop)

Callers must explicitly call `persist_and_release(guard)`. Guard drops without
resolution panic. Breaks `?` — every early return between guard creation and
`persist_and_release` triggers the panic.

### Explicit `persist_conversation` at every call site

Manual persist calls at every mutation site. Every new mutation path must
remember to add a call, and missing one is silent data loss.

## Non-Goals

- **Cross-process data sharing.** The `Arc<RwLock>` is process-local. Data
  sharing between processes goes through the filesystem, coordinated by the
  `flock`.

- **Multi-conversation locks.** Each lock targets one conversation.

## Risks and Open Questions

### Read guards across `.await`

`ArcRwLockReadGuard` is `Send`, so the compiler does not catch holding it
across `.await` — the code compiles but deadlocks at runtime. Mitigated by
convention: all lock guards are acquired, used, and dropped within a single
expression or block. Data needed after an `.await` is cloned first.

### Per-event persistence cost with `as_mut()`

Each `lock.as_mut()` creates a `ConversationMut` with a fresh `dirty` flag.
Calling `update_events` on it marks it dirty, and when it drops, it persists.
If used per-event in a loop, this causes one disk write per event. The correct
pattern batches mutations within a single `ConversationMut` scope:

```rust
let mut conv = lock.as_mut();
while let Some(event) = events.next().await {
    conv.update_events(|e| e.handle_event(event));
}
conv.flush()?;
```

## References

- [RFD 020]: Parallel Conversations — introduced conversation locking and
  session-based conversation tracking.
- [RFD 052]: Workspace Data Store Sanitization — discusses persistence safety.

[RFD 020]: 020-parallel-conversations.md
[RFD 052]: 052-workspace-data-store-sanitization.md
