# RFD 069: Guard-Scoped Persistence for Conversations

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-26
- **Extends**: [RFD 020]
- **Summary**: Persists modified conversation state automatically when
  `ConversationMut` drops while still holding the cross-process file lock.

## Summary

This RFD moves conversation persistence into `ConversationMut`'s `Drop`
implementation so modified data is written to disk automatically while the
cross-process `ConversationLock` is still held.
Callers derive mutable scopes from `ConversationLock` and can use `?` or early
returns without manual persist calls or lock-release race windows.

## Motivation

Persisting conversation data requires two guarantees: writes must complete while
the cross-process lock is held (preventing race windows), and every mutation
path must persist without manual calls (preventing silent data loss).
An explicit `persist_conversation()` API satisfies the first guarantee but not
the second: a missed call is a silent bug.
A `Workspace::Drop` safety net satisfies the second but not the first: by the
time `Workspace` drops, the lock has already been released.

Moving persistence into `ConversationMut`'s `Drop` implementation satisfies both
guarantees.
`ConversationMut` holds shared references to the conversation data via
`Arc<RwLock<...>>`, a storage write handle via `PersistBackend`, and shared
ownership of the file lock via `Arc`.
When the mutable scope ends, `Drop` writes modified data to disk before the file
lock is released.

## Design

### Per-Conversation Interior Mutability

Workspace state wraps each conversation's metadata and event stream in
`OnceLock<Arc<RwLock<...>>>`:

```rust
pub(super) struct State {
    pub(super) conversations: HashMap<ConversationId, OnceLock<Arc<RwLock<Conversation>>>>,
    pub(super) events: HashMap<ConversationId, OnceLock<Arc<RwLock<ConversationStream>>>>,
}
```

`OnceLock` loads each conversation from disk on first access.
`Arc` shares ownership between the workspace, active locks, and mutable scopes.
`RwLock` (`parking_lot::RwLock`) allows concurrent reads and exclusive writes
within the process.

Wrapping individual conversations rather than the entire `State` has three
benefits:

- **Per-conversation locking**: Accessing conversation A does not block access
  to conversation B.
- **Direct guard access**: Methods call `.read()` or `.write()` directly on the
  conversation's `RwLock` without projecting through a `MappedMutexGuard`.
- **Plain `HashMap` lookup**: `ConversationMut` tracks modifications with its
  `dirty` flag and persists on drop, and `remove_conversation_with_lock` deletes
  the conversation directory immediately, so `State` does not need a
  change-tracking map (`TombMap`).

`jp_workspace` never exposes the `Arc<RwLock<...>>` directly.
All public APIs return read guards or mutate through callbacks, preserving the
invariant that mutation requires holding the cross-process `flock`.

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

`ConversationLock` represents exclusive cross-process access to a conversation:
it proves that the process holds the `flock`, provides read access to metadata
and events, and produces `ConversationMut` scopes for writes.

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

`ConversationMut` is a mutable scope over a locked conversation that persists
modified data to disk when dropped.
Because it holds only `Arc` handles and an `AtomicBool` rather than lock guards,
`ConversationMut` is `Send + Sync` and can be held across `.await` points.

#### Callback-Based Mutation

Write methods take callbacks instead of returning raw `RwLockWriteGuard`s.
Bounding the write guard to the closure makes it structurally impossible to hold
a write lock across `.await` points:

```rust
impl ConversationMut {
    pub fn update_metadata<R>(&self, f: impl FnOnce(&mut Conversation) -> R) -> R;
    pub fn update_events<R>(&self, f: impl FnOnce(&mut ConversationStream) -> R) -> R;
    pub fn update<R>(&self, f: impl FnOnce(&mut Conversation, &mut ConversationStream) -> R) -> R;
}
```

Each method acquires the write guard for the duration of `f`, sets the dirty
flag unconditionally, and forwards `f`'s return value so `?` composes naturally:

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
    /// Returns a failure recorded by an earlier drop-time write before
    /// attempting its own.
    ///
    /// Takes `&mut self` to prevent calling while a write guard from
    /// update_events() is held (which would deadlock).
    pub fn flush(&mut self) -> Result<()>;
}

impl Drop for ConversationMut {
    fn drop(&mut self) {
        if !self.dirty.load(Ordering::Relaxed) { return; }

        let meta = self.metadata.read();
        let evts = self.events.read();
        if let Err(error) = self.writer.write(&self.id, &meta, &evts, self.projection) {
            warn!(id = %self.id, %error, "Failed to persist conversation.");
            let mut state = self.persist.lock();
            if state.failure.is_none() { state.failure = Some(error); }
        }
    }
}
```

Because `Drop` cannot return an error, a failed drop-time write records its
error in persistence state shared with the `Workspace` and the originating
`ConversationLock` rather than writing to the terminal.
`ConversationLock::take_persist_failure` (and its `ConversationMut` counterpart)
drains the recorded error so the shell reports it once through the printer and
exits with a non-zero code.
Only the first failure is retained, so a failing disk produces one diagnostic
rather than one per mutation scope.

Subsequent `ConversationMut` scopes still attempt to write on drop.
Persistence writes to two roots that may live on separate filesystems (the
durable user-local copy and the workspace projection), so a failure on one root
does not imply the other is unhealthy; skipping later writes would leave the
healthy root behind.

`flush` returns I/O errors directly to the caller without recording them in the
shared persistence state.
However, `flush` clears the dirty flag only when the write succeeds: if a caller
ignores an error from `flush`, the scope remains dirty, so `Drop` retries the
write and records any failure for later draining.

The `Workspace` owns the failure record, so it outlives both `ConversationMut`
and `ConversationLock`.
When a command future is cancelled mid-flight (Ctrl-C, SIGTERM), dropping the
future persists any active `ConversationMut` scope in `Drop` and drops the lock
with it, bypassing any drain call inside the future.
`Workspace::take_persist_failure` runs after the command future resolves or
drops, serving as the fallback drain for cancelled commands.

The dirty flag uses `AtomicBool` with `Ordering::Relaxed` instead of
`Cell<bool>`.
`Cell<bool>` is `!Sync`, which would make `ConversationMut` `!Sync` and make
async futures holding `&ConversationMut` across `.await` points `!Send`.

### `PersistBackend` Trait

`PersistBackend` abstracts disk writes so tests can verify persistence behavior
without disk I/O:

```rust
pub trait PersistBackend: Send + Sync + Debug {
    fn write(&self, id: &ConversationId, metadata: &Conversation,
             events: &ConversationStream) -> Result<()>;
    fn remove(&self, id: &ConversationId) -> Result<()>;
}
```

The production implementation (`FsPersistBackend`) captures the write paths from
`Storage` at construction time so `ConversationMut::Drop` can persist without
borrowing `Storage`.
Tests use `MockPersistBackend` to record calls, or `None` to skip persistence
entirely.

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
`ArcRwLockReadGuard` (iterator), both of which auto-deref to `&T`.
Neither exposes `.write()`; mutating a conversation requires acquiring a
`ConversationLock`.

`lock_conversation` takes `&self` because it only acquires the `flock` through
`Storage` and clones `Arc` pointers from `State`, without moving data out of the
workspace.

`remove_conversation_with_lock` consumes a `ConversationMut` by value, clears
its dirty flag so `Drop` does not persist data that is about to be deleted,
deletes the conversation directory immediately via `PersistBackend`, and removes
the conversation from the `HashMap` index.

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

The turn loop takes `&ConversationLock` and creates `ConversationMut` scopes as
needed.
Because `ConversationMut` is `Send + Sync`, it can be held across `.await`
points while each `update_events` callback confines its write guard to a
synchronous block:

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

Functions that mutate `ConversationStream` across `.await` points take
`&ConversationMut` and acquire the write lock per access via callbacks:

```rust
async fn execute_with_prompting(&mut self, conv: &ConversationMut, ...) {
    conv.update_events(|e| e.current_turn_mut().add_inquiry_response(resp));
    some_async_call().await;
    conv.update_events(|e| e.current_turn_mut().add_tool_response(resp));
}
```

### Data Visibility

Because `Workspace`, `ConversationLock`, and `ConversationMut` share the same
`Arc<RwLock<...>>`, readers can access conversation data at any time; no data is
checked out or hidden from the workspace.
Write locks are held only for individual callback invocations, so contention is
negligible.

### Test Support

`Workspace::test_lock` creates a lock backed by a no-op `flock`.
When the workspace has storage configured, the test lock automatically attaches
`FsPersistBackend` so tests that assert on-disk persistence work without extra
setup.
In-memory-only workspaces produce locks with `writer: None`, skipping
persistence entirely.

## Drawbacks

**Read-path API change.** `workspace.metadata(&handle)` returns
`RwLockReadGuard<Conversation>` instead of `&Conversation`.
Auto-deref makes most call sites transparent, but explicit type annotations need
adjustment, and callers must clone data and drop the guard before `.await`
points.

**Callback ergonomics.** Write access uses `conv.update_events(|e| ...)` instead
of `conv.events_mut().do_thing()`.
This is slightly more verbose, though forwarding the callback's return value
allows `?` to compose naturally while structurally preventing callers from
holding write guards across `.await` points.

**Errors in `Drop` cannot be propagated.** A persist failure during
`ConversationMut`'s drop cannot be returned from `drop`; instead, it is recorded
in the workspace-owned persistence state and surfaced by the next `flush()`, by
a `take_persist_failure()` drain in the command, or by the CLI's teardown drain
after a cancelled future.
Long-running loops must call `flush()?` at checkpoints so that I/O failures halt
immediately.

**`parking_lot` dependency.** `parking_lot::RwLock` is used instead of
`std::sync::RwLock` for non-poisoning locks, `DerefMut` on write guards, and
`ArcRwLockReadGuard` for `'static` lifetime guards.
`parking_lot` is already a transitive dependency.

**`Arc` overhead.** Wrapping each conversation's metadata and events in
`Arc<RwLock<...>>` adds a heap allocation and atomic reference count per
conversation, which is negligible for a typical workspace with tens to hundreds
of conversations.

## Alternatives

### Raw write guard access (no callbacks)

Exposing `events_mut()` and `metadata_mut()` methods that return
`RwLockWriteGuard` directly is more ergonomic for synchronous code, but
`RwLockReadGuard` (which is `Send`) can be held across `.await` points without
compiler errors, deadlocking silently at runtime.
Scoping write access to callbacks makes holding a write guard across `.await`
structurally impossible.

### Guard owns data (checkout model)

Moving conversation data into the guard for the duration of the lock achieves
auto-persistence on drop without `Arc<RwLock>`, but leaves the workspace entry
empty while the guard is alive, hiding the conversation from
`workspace.conversations()`.

### Whole-state locking (`Arc<Mutex<State>>`)

Wrapping the entire `State` in a single `Arc<Mutex<State>>` requires
`MappedMutexGuard` to project into a specific conversation and serializes all
access across unrelated conversations.

### Mandatory-resolution guard (panic-in-Drop)

Requiring callers to explicitly call `persist_and_release(guard)` and panicking
in `Drop` if they do not breaks `?`: every early return between guard creation
and `persist_and_release` triggers a panic.

### Explicit `persist_conversation` at every call site

Requiring manual `persist_conversation` calls at every mutation site forces
every new mutation path to remember to persist, and missing a call causes silent
data loss.

## Non-Goals

- **Cross-process data sharing.** The `Arc<RwLock>` is process-local.
  Data sharing between processes goes through the filesystem, coordinated by the
  `flock`.

- **Multi-conversation locks.** Each lock targets one conversation.

## Risks and Open Questions

### Read guards across `.await`

Because `ArcRwLockReadGuard` is `Send`, holding a read guard across `.await`
compiles without error but deadlocks at runtime.
By convention, callers acquire, read, and drop lock guards within a single
expression or block, cloning any data needed after an `.await`.

### Per-event persistence cost with `as_mut()`

Each `lock.as_mut()` call creates a `ConversationMut` with a fresh `dirty` flag
that persists on drop once `update_events` marks it dirty.
Calling `lock.as_mut()` per event in a loop causes one disk write per event.
Loops should instead batch mutations within a single `ConversationMut` scope:

```rust
let mut conv = lock.as_mut();
while let Some(event) = events.next().await {
    conv.update_events(|e| e.handle_event(event));
}
conv.flush()?;
```

## References

- [RFD 020] (Parallel Conversations) introduced conversation locking and
  session-based conversation tracking.
- [RFD 052] (Workspace Data Store Sanitization) discusses persistence safety.

[RFD 020]: 020-parallel-conversations.md
[RFD 052]: 052-workspace-data-store-sanitization.md
