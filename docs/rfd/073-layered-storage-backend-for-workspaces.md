# RFD 073: Layered Storage Backend for Workspaces

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-01
- **Extended by**: [RFD 106]
- **Summary**: Replaces `Workspace`'s optional `Storage` with non-optional
  `PersistBackend`, `LoadBackend`, `LockBackend`, and `SessionBackend` traits.

## Summary

This RFD replaces `Workspace`'s `Option<Storage>` with four non-optional trait
objects (`PersistBackend`, `LoadBackend`, `LockBackend`, and `SessionBackend`)
that decouple conversation persistence, loading, locking, and session management
from the filesystem.
`FsStorageBackend` implements all four traits for on-disk workspaces,
`InMemoryStorageBackend` provides an in-memory backend for tests, and
`NullPersistBackend` handles `--no-persist` without optional fields in
`Workspace`, `ConversationLock`, or `ConversationMut`.

## Motivation

`Workspace` currently holds an `Option<Storage>`, and downstream types carry the
consequences: `persist_backend` is `Option<Arc<dyn PersistBackend>>`,
`ConversationLock` holds `file_lock: Option<Arc<ConversationFileLock>>`, and
`ConversationMut` holds `writer: Option<Arc<dyn PersistBackend>>`.

At least 15 methods on `Workspace` branch on `self.storage.as_ref()`, with three
different behaviors when storage is absent:

- **No-op:** `load_conversation_index`, `ensure_all_metadata_loaded`,
  `cleanup_stale_files`, `remove_ephemeral_conversations`, `conversations`
  (skips lazy loading), `metadata`/`events` (skip disk initialization).
- **Return an error:** `eager_load_conversation`, `lock_conversation`,
  `sanitize`, `activate_session_conversation`.
- **Return `None`:** `conversation_lock_info`, `storage_path`,
  `user_storage_path`, `load_session_mapping`.

Callers must reason about whether storage exists and which failure mode applies
to each method.

The `PersistBackend` trait ([RFD 069]) abstracts conversation `write` and
`remove` operations, but loading, indexing, locking, and session management
still call `Storage` directly.
`Workspace` therefore still holds `Option<Storage>` alongside `PersistBackend`.

A `disable_persistence` boolean on `Workspace` adds another layer of optionality
and serves three distinct purposes:

1. **`--no-persist` CLI flag (production).** Users run `jp -! query` for
   ephemeral one-off queries, or to try different approaches to the next turn
   without committing to the conversation history.
   This reads from the filesystem as normal, but does not write mutations back.

2. **Error-path safety (production).** When a command fails, the CLI calls
   `workspace.disable_persistence()` before background-task sync to prevent
   writing potentially corrupt state.
   The `query` command opts out of this for turn errors so partial results are
   still persisted.

3. **Test convenience.** Tests call `disable_persistence()` to avoid creating
   temporary directories because there is no in-memory storage backend.

`InMemoryStorageBackend` eliminates the test workaround directly.
The two production cases require a hybrid configuration that loads and locks on
the filesystem without persisting writes.
Splitting storage into separate traits supports this through backend
composition: the `persist` backend can be swapped independently of the other
three, so `Workspace` always holds a storage backend even when it is not backed
by the filesystem.

### Concrete Pain Points

**Tests require temporary directories.** Every test that needs a `Workspace`
with persistence creates a `tempdir`, constructs `Storage`, and calls
`persisted_at`.
Tests that do not need persistence use `Workspace::new(Utf8PathBuf::new())` with
no storage, which means they cannot lock conversations, persist data, or load
from disk.

**`lock_new_conversation` branches on storage existence.** When creating and
locking a conversation, the method acquires a `flock` if storage exists and
skips locking if it does not.
In-memory workspaces therefore have no locking, not even in-process mutual
exclusion.
`ConversationLock` has the same type in both cases, whether or not it holds an
actual lock.

**`ConversationMut` silently skips persistence.** If `writer` is `None` (no
storage) or `disable_persistence` is `true`, the drop handler is a no-op, and a
caller holding a `ConversationMut` cannot tell whether its mutations will be
persisted.
`disable_persistence` also branches in two places: `Workspace` checks it when
constructing locks (passing `writer: None` when set), and `ConversationMut`
checks `if let Some(writer)` on every flush and drop.
With backend polymorphism, `writer` is always present and `NullPersistBackend`
handles the no-op case through the trait.

## Design

### Trait Decomposition

`Storage`'s responsibilities split into four traits, one per concern:

#### `PersistBackend`

`PersistBackend` ([RFD 069]) writes and removes conversation data and is
unchanged:

```rust
pub trait PersistBackend: Send + Sync + Debug {
    fn write(
        &self,
        id: &ConversationId,
        metadata: &Conversation,
        events: &ConversationStream,
    ) -> Result<()>;

    fn remove(&self, id: &ConversationId) -> Result<()>;
}
```

#### `LoadBackend`

`LoadBackend` reads conversation metadata, event streams, and indexes from the
backing store:

```rust
pub trait LoadBackend: Send + Sync + Debug {
    /// Scan all conversation IDs from the backing store.
    fn load_all_conversation_ids(&self) -> Vec<ConversationId>;

    /// Load a single conversation's metadata.
    fn load_conversation_metadata(
        &self,
        id: &ConversationId,
    ) -> Result<Conversation, LoadError>;

    /// Load a single conversation's event stream.
    fn load_conversation_stream(
        &self,
        id: &ConversationId,
    ) -> Result<ConversationStream, LoadError>;

    /// Return conversation IDs whose `expires_at` timestamp is in the past.
    ///
    /// `FsStorageBackend` implements this with a fast-path JSON reader that
    /// extracts only the `expires_at` field from `metadata.json` without
    /// deserializing the full `Conversation` struct
    /// (see `get_expiring_timestamp` in `jp_storage`).
    ///
    /// `InMemoryStorageBackend` checks the in-memory `Conversation` structs
    /// directly.
    fn load_expired_conversation_ids(
        &self,
        now: DateTime<Utc>,
    ) -> Vec<ConversationId>;

    /// Validate and repair the backing store.
    ///
    /// For filesystem backends, this scans conversation directories, trashes
    /// corrupt entries to `.trash/`, and returns a report of what was
    /// repaired. For in-memory backends, data is always structurally valid,
    /// so this returns an empty report.
    ///
    /// This should be called before `load_conversation_index` to guarantee
    /// the store is in a consistent state.
    fn sanitize(&self) -> Result<SanitizeReport>;
}
```

#### `LockBackend`

`LockBackend` abstracts conversation-level locking, which `Storage` currently
implements with `flock`:

```rust
pub trait LockBackend: Send + Sync + Debug {
    /// Attempt to acquire an exclusive lock on a conversation.
    ///
    /// Returns `Ok(Some(lock))` if acquired, `Ok(None)` if another holder
    /// has it, or `Err` on infrastructure failure.
    fn try_lock(
        &self,
        conversation_id: &str,
        session: Option<&str>,
    ) -> Result<Option<Box<dyn ConversationLockGuard>>>;

    /// Read diagnostic info about a lock holder.
    fn lock_info(&self, conversation_id: &str) -> Option<LockInfo>;

    /// List conversation IDs with orphaned locks (not held by any process).
    fn list_orphaned_locks(&self) -> Vec<ConversationId>;
}

/// A held conversation lock. Released on drop.
pub trait ConversationLockGuard: Send + Sync + Debug {}
```

The `ConversationFileLock` struct becomes the filesystem implementation of
`ConversationLockGuard`.
In-memory backends use an in-process mutex-based guard.

#### `SessionBackend`

`SessionBackend` loads, saves, and lists session-to-conversation mappings:

```rust
pub trait SessionBackend: Send + Sync + Debug {
    /// Load a session mapping.
    fn load_session<T: DeserializeOwned>(&self, session_key: &str) -> Result<Option<T>>;

    /// Save a session mapping.
    fn save_session<T: Serialize>(&self, session_key: &str, data: &T) -> Result<()>;

    /// List all session mapping keys.
    fn list_session_keys(&self) -> Vec<String>;
}
```

### `Workspace` Structure

The new `Workspace` holds all four backends as non-optional trait objects:

```rust
pub struct Workspace {
    root: Utf8PathBuf,
    id: Id,
    persist: Arc<dyn PersistBackend>,
    loader: Arc<dyn LoadBackend>,
    locker: Arc<dyn LockBackend>,
    sessions: Arc<dyn SessionBackend>,
    state: State,
}
```

This replaces `Option<Storage>` and the `disable_persistence` boolean field.
Methods that previously branched on `self.storage` now dispatch through the four
traits.
`Workspace::disable_persistence()` remains for existing CLI call sites, swapping
`self.persist` for a `NullPersistBackend` instead of flipping a boolean.

`root` remains on `Workspace` because the project root directory is independent
of conversation storage.
A future `ProjectFiles` abstraction may revisit whether `root` belongs on
`Workspace`.

### `FsStorageBackend`

`FsStorageBackend` implements all four traits by delegating to `Storage`:

```rust
#[derive(Debug, Clone)]
pub struct FsStorageBackend {
    storage: Storage,
}

impl PersistBackend for FsStorageBackend { /* delegates to storage */ }
impl LoadBackend for FsStorageBackend { /* delegates to storage */ }
impl LockBackend for FsStorageBackend { /* delegates to storage */ }
impl SessionBackend for FsStorageBackend { /* delegates to storage */ }
```

`Storage` becomes an internal detail of `jp_storage`, and external callers use
the backend traits instead.
Filesystem-specific accessors such as `storage_path()` and `user_storage_path()`
stay on `FsStorageBackend` rather than the traits, for callers that need on-disk
paths (config loading and editor file placement).

### `NullPersistBackend`

`NullPersistBackend` is a no-op `PersistBackend` that discards all writes, used
for ephemeral mode (`--no-persist`) and error-path persistence suppression:

```rust
#[derive(Debug)]
pub struct NullPersistBackend;

impl PersistBackend for NullPersistBackend {
    fn write(
        &self,
        _id: &ConversationId,
        _metadata: &Conversation,
        _events: &ConversationStream,
    ) -> Result<()> {
        Ok(())
    }

    fn remove(&self, _id: &ConversationId) -> Result<()> {
        Ok(())
    }
}
```

`NullPersistBackend` implements only `PersistBackend`.
When the CLI needs filesystem reads without writes, it pairs
`NullPersistBackend` with `FsStorageBackend` for loading, locking, and sessions.

### `InMemoryStorageBackend`

`InMemoryStorageBackend` stores conversations, locks, and sessions in memory for
tests and non-filesystem environments:

```rust
#[derive(Debug, Default)]
pub struct InMemoryStorageBackend {
    conversations: Mutex<HashMap<ConversationId, (Conversation, ConversationStream)>>,
    locks: Mutex<HashSet<String>>,
    sessions: Mutex<HashMap<String, Vec<u8>>>,
}

impl PersistBackend for InMemoryStorageBackend { /* writes to conversations map */ }
impl LoadBackend for InMemoryStorageBackend { /* reads from conversations map */ }
impl LockBackend for InMemoryStorageBackend { /* in-process mutex-based locking */ }
impl SessionBackend for InMemoryStorageBackend { /* reads/writes sessions map */ }
```

`try_lock` tracks held conversation IDs in an in-memory `HashSet`, providing
single-process mutual exclusion without filesystem `flock`.
Cross-process locking does not apply to in-memory stores.

### Ephemeral Mode (`--no-persist`)

The `--no-persist` flag requires reading from the filesystem (to load existing
conversations, config, and session mappings) while discarding writes and
skipping lock contention.
The CLI constructs a mixed set of backends in `load_workspace` before passing
them to `Workspace`:

- `persist: Arc<NullPersistBackend>`: discards writes silently.
- `loader: Arc<FsStorageBackend>`: reads conversations from disk as normal.
- `locker: Arc<NullLockBackend>`: succeeds immediately on every lock attempt, so
  ephemeral queries never block on lock contention.
- `sessions: Arc<FsStorageBackend>`: continues to update session mappings
  (`activate_session_conversation` runs regardless of `--no-persist`).

Selecting these backends at construction time avoids a runtime flag:

```rust
let fs = Arc::new(FsStorageBackend::new(&storage_path)?
    .with_user_storage(&user_root, name, id.to_string())?);

let mut workspace = Workspace::new_with_id(root, id).with_backend(fs.clone());
if !persist {
    workspace = workspace
        .with_persist(Arc::new(NullPersistBackend))
        .with_locker(Arc::new(NullLockBackend));
}
```

For the error-path case, where `disable_persistence()` is called *after*
construction, `Workspace::disable_persistence()` swaps the `persist` field:

```rust
impl Workspace {
    pub fn disable_persistence(&mut self) {
        self.persist = Arc::new(NullPersistBackend);
    }
}
```

Existing `ConversationMut` instances hold their own `Arc` clone of the original
backend and still persist on drop.
Because the query's `ConversationLock` is dropped before the error check runs,
swapping `self.persist` only affects subsequent operations such as background
task sync and ephemeral cleanup, matching the existing behavior.

### `ConversationLock` and `ConversationMut` Changes

Both types hold non-optional backend and lock-guard fields:

```rust
pub struct ConversationLock {
    id: ConversationId,
    metadata: Arc<RwLock<Conversation>>,
    events: Arc<RwLock<ConversationStream>>,
    writer: Arc<dyn PersistBackend>,            // was Option<Arc<dyn PersistBackend>>
    lock_guard: Box<dyn ConversationLockGuard>, // was Option<Arc<ConversationFileLock>>
}

pub struct ConversationMut {
    id: ConversationId,
    metadata: Arc<RwLock<Conversation>>,
    events: Arc<RwLock<ConversationStream>>,
    dirty: AtomicBool,
    writer: Arc<dyn PersistBackend>,              // was Option<Arc<dyn PersistBackend>>
    _lock_guard: Box<dyn ConversationLockGuard>,  // was Option<Arc<ConversationFileLock>>
}
```

Both `writer` and `lock_guard` are always present.
`ConversationMut::flush` and `Drop` call `writer.write()` unconditionally
instead of branching on `Option<writer>` and `disable_persistence`, and
`lock_new_conversation` and `lock_conversation` always acquire a guard.
`InMemoryStorageBackend` writes to its in-memory map (which tests can inspect)
and releases its in-process lock on drop; `NullPersistBackend` makes `write()` a
no-op for ephemeral mode.

### Validation and Sanitization

`Workspace::sanitize()` ([RFD 052]) replaces
`self.storage.as_ref().ok_or(Error::MissingStorage)?` with a call to
`LoadBackend::sanitize()`, keeping its `Result<SanitizeReport>` signature so the
CLI code that warns about trashed conversations works unchanged:

```rust
impl Workspace {
    pub fn sanitize(&mut self) -> Result<SanitizeReport> {
        self.loader.sanitize().map_err(Into::into)
    }
}
```

`FsStorageBackend` delegates to `Storage`'s directory scan and `.trash/`
recovery logic.
`InMemoryStorageBackend` returns `Ok(SanitizeReport::default())`, since
in-memory data is written through typed APIs and is always structurally valid.
Keeping `sanitize()` on `LoadBackend` avoids moving sanitization into a
pre-construction step that would require filesystem-specific branching in the
CLI.

### Ephemeral Conversation Cleanup

`Storage::remove_ephemeral_conversations` scans conversation directories for
`expires_at` timestamps, combining metadata reads with deletion.
This method moves to `Workspace`, calling
`LoadBackend::load_expired_conversation_ids` to find expired conversations and
`PersistBackend::remove` to delete them.

`LoadBackend` provides a dedicated `load_expired_conversation_ids` method so
`FsStorageBackend` can keep using `Storage`'s private `get_expiring_timestamp`
helper, which extracts **only** the `expires_at` field from raw `metadata.json`
without deserializing the full `Conversation` struct.
Iterating through `load_conversation_metadata` instead would deserialize every
conversation's metadata into memory just to check its expiration timestamp.
`InMemoryStorageBackend` reads its in-memory `Conversation` structs directly.

### Filesystem Path Methods

`Workspace` exposes several methods that return filesystem paths for individual
conversations:

- `build_conversation_dir(id, title, user)`: constructs the expected directory
  path.
- `conversation_dir(id)`: finds an existing directory on disk.
- `conversation_events_path(id)`: returns the path to `events.json`.
- `conversation_metadata_path(id)`: returns the path to `metadata.json`.
- `conversation_base_config_path(id)`: returns the path to `base_config.json`.

The CLI uses these methods for editor integration (`jp conversation edit`), the
`jp conversation path` command, config loading (reading `base_config.json`), and
plugin dispatch (which reads `storage_path` and `user_storage_path`).
Because they are filesystem-specific, they are removed from `Workspace` and live
on `FsStorageBackend` instead.
The CLI retains a typed `Arc<FsStorageBackend>` in its `Ctx` struct and calls
the path methods on the backend directly:

```rust
// jp_cli/src/ctx.rs
pub(crate) struct Ctx {
    pub(crate) workspace: Workspace,

    /// Typed reference to the filesystem backend for path queries,
    /// config loading, and other fs-specific operations.
    /// `None` when running with an in-memory backend (tests).
    pub(crate) fs_backend: Option<Arc<FsStorageBackend>>,

    // ... other fields ...
}
```

For example, `jp conversation path` calls
`ctx.fs_backend.as_ref()?.conversation_dir(id)` instead of
`ctx.workspace.conversation_dir(id)`.
Commands that require filesystem paths (such as `conversation edit` and
`conversation path`) return a CLI-level error when `fs_backend` is `None`.
Similarly, `load_partial_configs_from_files` takes `Option<&FsStorageBackend>`
instead of `Option<&Workspace>` to read `storage_path()` and
`user_storage_path()`.

This keeps filesystem concerns in `jp_cli` while `Workspace` depends only on the
four backend traits.

### Error Type Boundary

`PersistBackend` moves from `jp_workspace::persist` to `jp_storage` (Phase 1),
and its methods return `jp_storage::error::Result` instead of
`jp_workspace::error::Result` because `jp_storage` cannot depend on
`jp_workspace`.
Because `jp_workspace::Error` already wraps `jp_storage::Error` via `#[from]`,
callers in `jp_workspace` (such as `ConversationMut::flush`) convert errors with
`?`.

### Construction Pattern

The current builder chain:

```rust
Workspace::new(root)
    .persisted_at(&storage)?     // creates Storage, sets self.storage
    .with_local_storage()?       // mutates Storage with user storage
```

is replaced by constructing the backend first, then passing it to `Workspace`:

```rust
// CLI construction (production, filesystem)
let fs = Arc::new(
    FsStorageBackend::new(&storage_path)?
        .with_user_storage(&user_root, name, id)?,
);

let mut workspace = Workspace::new_with_id(root, id).with_backend(fs.clone());
let ctx = Ctx::new(workspace, Some(fs), ...);

// CLI construction (ephemeral mode, --no-persist)
let fs = Arc::new(
    FsStorageBackend::new(&storage_path)?
        .with_user_storage(&user_root, name, id)?,
);

let mut workspace = Workspace::new_with_id(root, id)
    .with_backend(fs.clone())
    .with_persist(Arc::new(NullPersistBackend))
    .with_locker(Arc::new(NullLockBackend));

let ctx = Ctx::new(workspace, Some(fs), ...);

// Test construction (in-memory)
let workspace = Workspace::new(root);
// All four backends default to a single shared InMemoryStorageBackend.
```

`FsStorageBackend::new` and `with_user_storage` encapsulate `Storage`
construction and user-storage setup, and the fluent builder methods on
`Workspace` (`with_backend`, `with_persist`, `with_locker`, etc.) make backend
selection explicit.

## Drawbacks

**Four trait objects instead of one `Option<Storage>`.** Holding four `Arc<dyn
Trait>` fields adds indirection and dynamic dispatch, though the cost is
negligible because these methods run at I/O boundaries rather than in hot loops.

**Trait surface area.** Four traits with roughly 18 methods in total must be
updated across the trait definitions, both implementations, and any mocks when
adding a storage operation.
These methods already exist as concrete methods on `Storage`, so the traits make
an existing surface explicit.

**`FsStorageBackend` wraps `Storage` rather than replacing it.** `Storage`
remains inside `jp_storage` because its directory scanning, symlink management,
and path resolution are filesystem-specific.
Inlining `Storage` into `FsStorageBackend` would only move that code without
simplifying it.

## Alternatives

### Single Wide `StorageBackend` Trait

Combining all ~15 methods into one trait requires only a single trait object on
`Workspace`, but it couples unrelated concerns.
A mock that only verifies persistence would still have to implement locking,
loading, and session methods, whereas separate traits let tests mock only the
concern under test.

### Enum Dispatch Instead of Trait Objects

```rust
enum StorageBackend { Fs(FsStorage), InMemory(InMemoryStorage) }
```

Enum dispatch avoids `dyn` overhead and `Arc` wrapping, but it is a closed set:
every new backend requires modifying the enum.
Trait objects allow adding planned browser/Web Storage backends without changing
`jp_storage` or `jp_workspace`.

### Keep `Option<Storage>` and Widen `PersistBackend`

Incrementally adding methods to `PersistBackend` and removing `Option<Storage>`
only once the trait covers all of `Storage` would leave the codebase split
between trait calls and `Option<Storage>` branches during the transition.
Introducing the four traits replaces `Option<Storage>` in a single step.

### Make `Storage` Non-Optional Without Traits

Requiring a concrete `Storage` in `Workspace::new` and using `tempdir` in all
tests eliminates the `Option<Storage>` branching, but it still ties `Workspace`
to the filesystem and prevents in-memory, browser, or database backends.

## Non-Goals

- **Database or browser storage backends.** This RFD defines the traits and
  provides filesystem and in-memory implementations.
  Other backends are future work.

- **Project file abstraction.** Whether `Workspace::root` should be replaced by
  a backend-agnostic `ProjectFiles` abstraction is a separate concern, addressed
  in a subsequent RFD.

- **Config loading abstraction.** The config pipeline reads from multiple
  filesystem roots (user-global, workspace, user-workspace).
  Abstracting this is out of scope.

- **Changes to `Storage`'s internal logic.** Directory scanning, conversation
  persistence, and symlink management remain unchanged inside `Storage`; this
  RFD only wraps them behind traits.

## Risks and Open Questions

### Trait Method Granularity

The proposed traits group methods by concern (persist, load, lock, session)
rather than by entity (conversation backend, session backend).
Grouping by concern matches how `Workspace` branches on storage today (such as
swapping only persistence for `--no-persist`), though implementation may reveal
a more natural split.

### Shared vs. Separate Backend Instances

`FsStorageBackend` implements all four traits on a single struct.
An alternative is four separate structs, each implementing one trait, which
allows mixing independent filesystem implementations per trait at the cost of
extra construction boilerplate.
Starting with a single struct and splitting later if needed is lower risk.

### Sanitization for Non-Filesystem Backends

Sanitization currently scans conversation directories and moves corrupt entries
to `.trash/`.
Other backends have different integrity checks: `InMemoryStorageBackend` needs
none and returns an empty `SanitizeReport`, while a future database backend
would run SQL integrity checks rather than scanning directories.
Placing `sanitize()` on `LoadBackend` lets each backend define its own
validation.

## Implementation Plan

### Phase 1: Define traits in `jp_storage`

Move `PersistBackend` from `jp_workspace::persist` to `jp_storage` and define
`LoadBackend`, `LockBackend`, `SessionBackend`, and `ConversationLockGuard`,
without implementations or callers yet.

**Depends on:** Nothing.
**Mergeable:** Yes.

### Phase 2: `FsStorageBackend`

Implement all four traits for `FsStorageBackend` by delegating to the existing
`Storage` methods, with no behavior changes.

**Depends on:** Phase 1.
**Mergeable:** Yes.

### Phase 3: `InMemoryStorageBackend`

Implement all four traits for `InMemoryStorageBackend` using in-memory data
structures.
Add tests that exercise the same operations against both backends to verify
behavioral equivalence.

**Depends on:** Phase 1.
**Mergeable:** Yes (can be done in parallel with Phase 2).

### Phase 4: Refactor `Workspace`

Replace `Option<Storage>` with the four trait objects and reimplement
`Workspace::disable_persistence()` to swap `self.persist` for
`NullPersistBackend`.
Update all `Workspace` methods that branched on `self.storage.as_ref()` to
dispatch through the traits, and make `writer` and `lock_guard` non-optional on
`ConversationLock` and `ConversationMut`.

Update the CLI's `--no-persist` path to construct the workspace with
`NullPersistBackend` instead of calling `disable_persistence()` after
construction.
Error-path callers continue to call `disable_persistence()` after construction.

**Depends on:** Phase 2 and Phase 3.
**Mergeable:** Yes, as a single large PR that touches `Workspace`,
`ConversationLock`, `ConversationMut`, and their callers.

### Phase 5: Migrate tests

Replace test code that creates `Workspace::new(Utf8PathBuf::new())` (no storage)
with `Workspace::new(id, &InMemoryStorageBackend::default())`.
Replace tests that create tempdir-backed workspaces and call
`disable_persistence()` with `InMemoryStorageBackend`.

**Depends on:** Phase 4.
**Mergeable:** Yes.

### Phase 6: Remove dead code

Remove `MockPersistBackend` from `jp_workspace::persist` (replaced by
`InMemoryStorageBackend`), the root-only `Workspace::new()` constructor
(replaced by the backend-accepting constructors), `#[cfg(debug_assertions)]`
test helpers that worked around optional storage, and the `disable_persistence`
boolean field on `Workspace`.

**Depends on:** Phase 5.
**Mergeable:** Yes.

## References

- [RFD 020]: Parallel Conversations.
  Introduced conversation locking and session-based tracking.
- [RFD 031]: Durable Conversation Storage with Workspace Projection.
  Redesigns the storage model around user-local storage as the source of truth.
- [RFD 052]: Workspace Data Store Sanitization.
  Defines `Workspace::sanitize()`, which this RFD moves behind `LoadBackend`.
- [RFD 069]: Guard-Scoped Persistence for Conversations.
  Introduced `PersistBackend` and `ConversationMut` auto-persist, which this RFD
  builds on.

[RFD 020]: 020-parallel-conversations.md
[RFD 031]: 031-durable-conversation-storage-with-workspace-projection.md
[RFD 052]: 052-workspace-data-store-sanitization.md
[RFD 069]: 069-guard-scoped-persistence-for-conversations.md
[RFD 106]: 106-collision-free-conversation-id-allocation.md
