# RFD 073: Layered Storage Backend for Workspaces

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-01

## Summary

This RFD replaces `Workspace`'s `Option<Storage>` with a set of non-optional
trait objects that decouple conversation persistence, loading, locking, and
session management from the filesystem. Four focused traits — `PersistBackend`
(widened from today's version), `LoadBackend`, `LockBackend`, and
`SessionBackend` — replace the single `Storage` struct. `FsStorageBackend`
implements all four, wrapping the current `Storage` behavior.
`InMemoryStorageBackend` provides a filesystem-free implementation for tests and
future non-filesystem environments. A `NullPersistBackend` provides a no-op
`PersistBackend` for ephemeral mode (`--no-persist`). `Workspace` always holds
all four backends; all `Option<Storage>` branching is eliminated. The
`disable_persistence` method is retained but its implementation changes from
flipping a boolean to swapping the `persist` backend for `NullPersistBackend`.
`ConversationLock` and `ConversationMut` always have a writer and a lock
backend, removing their internal optionality as well.

## Motivation

`Workspace` currently holds an `Option<Storage>`, and downstream types carry
the consequences: `persist_backend` is `Option<Arc<dyn PersistBackend>>`,
`ConversationLock` holds `file_lock: Option<Arc<ConversationFileLock>>`, and
`ConversationMut` holds `writer: Option<Arc<dyn PersistBackend>>`. A
`disable_persistence` boolean adds a third dimension of optionality.

This optional storage creates pervasive branching throughout the workspace
layer. At least 15 methods on `Workspace` branch on `self.storage.as_ref()`:

- **No-op if missing:** `load_conversation_index`, `ensure_all_metadata_loaded`,
  `cleanup_stale_files`, `remove_ephemeral_conversations`, `conversations` (skips
  lazy loading), `metadata`/`events` (skip disk initialization).
- **Error if missing:** `eager_load_conversation`, `lock_conversation`,
  `sanitize`, `activate_session_conversation`.
- **Returns `None`:** `conversation_lock_info`, `storage_path`,
  `user_storage_path`, `load_session_mapping`.

Each of these branches represents a case where the caller must reason about
whether storage exists. Some methods silently do nothing, others return errors,
and others return `None` — three different failure modes for the same underlying
condition.

The `PersistBackend` trait ([RFD 069]) was a first step toward abstracting
storage, but its scope is narrow: only `write` and `remove` for conversations.
Loading, indexing, locking, and session management still go directly through
`Storage`, which is why `Option<Storage>` persists alongside the trait.

The `disable_persistence` flag adds a third dimension of optionality. It
serves three distinct purposes today:

1. **`--no-persist` CLI flag (production).** Users run `jp -! query` for
   ephemeral one-off queries, or to try different approaches to the next turn
   without committing to the conversation history. The semantics are: read
   from the filesystem as normal, but don't write mutations back.

2. **Error-path safety (production).** When a command fails, the CLI calls
   `workspace.disable_persistence()` before background-task sync to prevent
   writing potentially corrupt state. The query command explicitly opts out
   of this for turn errors (partial results should still be persisted).

3. **Test convenience.** Tests call `disable_persistence()` to avoid needing
   temporary directories. This is a workaround for the real problem: there
   is no in-memory storage backend.

Use case #3 is a workaround that `InMemoryStorageBackend` eliminates
directly. Use cases #1 and #2, however, are legitimate production behaviors
that require a hybrid model: filesystem-backed loading and locking, but no
persistence. The trait decomposition in this RFD enables this naturally
through backend composition — the `persist` trait object can be swapped
independently of the other three.

The goal is to replace optionality with polymorphism. `Workspace` always has
storage — it just doesn't always have *filesystem* storage.

### Concrete pain points

**Tests require temporary directories.** Every test that needs a `Workspace`
with persistence currently creates a `tempdir`, constructs `Storage`, and
calls `persisted_at`. Tests that don't need persistence use
`Workspace::new(Utf8PathBuf::new())` with no storage — but these workspaces
can't lock conversations, persist data, or load from disk. There is no middle
ground.

**`lock_new_conversation` branches on storage existence.** When creating and
locking a conversation, the method tries to acquire a flock if storage exists,
and skips locking if it doesn't. This means in-memory workspaces have no
locking at all — not even in-process mutual exclusion. The type system
doesn't reflect this: `ConversationLock` looks the same in both cases, but
one has a real lock and the other doesn't.

**`ConversationMut` silently skips persistence.** If `writer` is `None` (no
storage) or `disable_persistence` is `true`, the drop handler does nothing.
This is correct for tests and `--no-persist`, but the code path is
invisible — a caller holding a `ConversationMut` has no way to know whether
their mutations will be persisted. The `disable_persistence` boolean creates
branching at two levels: the `Workspace` checks it when constructing locks
(to decide whether to pass `writer: None`), and `ConversationMut` checks
`if let Some(writer)` on every flush and drop. With backend polymorphism,
both checks collapse: the writer is always present, and a
`NullPersistBackend` handles the "don't persist" case through the trait.

## Design

### Trait Decomposition

The current `Storage` struct's responsibilities split into four traits, each
covering a distinct concern:

#### `PersistBackend`

Writes and removes conversation data. This trait already exists ([RFD 069])
and is unchanged:

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

Reads conversation data and indexes. Covers the loading side of what `Storage`
does today:

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

Conversation-level locking. Today this is `flock`-based; the trait abstracts
over the mechanism:

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
`ConversationLockGuard`. In-memory backends use an in-process mutex-based
guard.

#### `SessionBackend`

Session-to-conversation mapping storage:

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

`Option<Storage>` is gone. Every method that previously branched on storage
existence now calls through the trait. The `disable_persistence` boolean is
also gone, but the `disable_persistence()` method is retained — its
implementation swaps the `persist` trait object for a `NullPersistBackend`
instead of flipping a boolean. This preserves the existing call sites in
the CLI (`--no-persist` flag, error-path safety) without any branching in
`Workspace` internals.

`root` is retained. It represents the project root directory and is
orthogonal to the storage backend abstraction. A subsequent RFD may
introduce a `ProjectFiles` abstraction and revisit whether `root` belongs
on `Workspace`, but that is out of scope here.

### `FsStorageBackend`

A single struct that implements all four traits, wrapping the current `Storage`
logic:

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

`Storage` itself remains as an internal implementation detail within
`jp_storage`. Its public API surface shrinks — external code interacts with
the traits, not `Storage` directly. The `storage_path()` and
`user_storage_path()` methods remain available on `FsStorageBackend` for
callers that need filesystem paths (config loading, editor file placement).
They are not part of the trait — they are filesystem-specific.

### `NullPersistBackend`

A trivial `PersistBackend` that silently discards all writes. Used for
ephemeral mode (`--no-persist`) and error-path persistence suppression:

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

`NullPersistBackend` only implements `PersistBackend`, not the other three
traits. It is composed with `FsStorageBackend` (for load, lock, and session)
when the CLI needs filesystem reads but no writes.

### `InMemoryStorageBackend`

A purely in-memory implementation for tests and non-filesystem environments:

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

Locking in the in-memory backend uses an in-process check: `try_lock` succeeds
if no other holder has the conversation ID in the locks set. This provides the
same mutual exclusion semantics within a single process, without filesystem
`flock`. Cross-process locking is not applicable for in-memory backends.

### Ephemeral Mode (`--no-persist`)

The `--no-persist` flag requires reading from the filesystem (to load existing
conversations, config, session mappings) while discarding all writes and
skipping lock contention. The trait decomposition handles this through mixed
backend construction:

- `persist: Arc<NullPersistBackend>` — writes are silently discarded
- `loader: Arc<FsStorageBackend>` — reads from disk as normal
- `locker: Arc<NullLockBackend>` — every lock attempt succeeds immediately, so
  ephemeral queries never block on lock contention
- `sessions: Arc<FsStorageBackend>` — session mapping updates still occur
  (current behavior: `activate_session_conversation` runs regardless of
  `--no-persist`)

The CLI constructs this mixed configuration in `load_workspace` before passing
backends to `Workspace`. No runtime flag is needed — the decision is baked into
the backend selection at construction time:

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

Already-created `ConversationMut` instances hold their own `Arc` clone of
the original backend, so they still persist on drop. This is correct: the
query's `ConversationLock` is dropped before the error check runs. The swap
only affects subsequent operations (background task sync, ephemeral cleanup),
which matches the current semantics exactly.

### `ConversationLock` and `ConversationMut` Changes

Both types lose their internal optionality:

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

The `writer` is always present. The `lock_guard` is always present. The
`InMemoryStorageBackend`'s persist implementation writes to its internal map;
its lock guard releases the in-process mutex on drop. No branching needed.

`ConversationMut::flush` and `Drop` no longer check `if let Some(writer)` —
they always call `writer.write()`. For `InMemoryStorageBackend`, this writes
to the in-memory map (useful for test assertions). For `NullPersistBackend`,
this is a no-op (ephemeral mode). The branching that currently exists in
`flush`, `Drop`, `lock_new_conversation`, and `lock_conversation` — checking
both `Option<writer>` and `disable_persistence` — collapses into a single
trait dispatch.

### Validation and Sanitization

`Workspace::sanitize()` ([RFD 052]) currently calls `self.storage.as_ref()
.ok_or(Error::MissingStorage)?`. With the new model, `Workspace::sanitize()`
delegates to `LoadBackend::sanitize()`, which returns a `SanitizeReport`.

For `FsStorageBackend`, this delegates to the existing `Storage` validation
and trash logic (directory scanning, moving corrupt entries to `.trash/`).
For `InMemoryStorageBackend`, this trivially returns
`Ok(SanitizeReport::default())` — in-memory data is always structurally
valid because it was written through typed APIs.

`Workspace::sanitize()` retains its current signature and continues to
return `SanitizeReport`. The CLI's existing code — printing warnings for
trashed conversations — works unchanged:

```rust
impl Workspace {
    pub fn sanitize(&mut self) -> Result<SanitizeReport> {
        self.loader.sanitize().map_err(Into::into)
    }
}
```

This avoids splitting sanitization into a pre-construction step that would
force filesystem-specific branching into the CLI initialization logic.

### Ephemeral Conversation Cleanup

`Storage::remove_ephemeral_conversations` scans conversation directories for
`expires_at` timestamps. This is a persistence concern with a loading
dependency (it needs to read metadata to check timestamps).

This moves to a method on `Workspace` that uses
`LoadBackend::load_expired_conversation_ids` to find expired conversations
and `PersistBackend::remove` to delete them.

The dedicated `load_expired_conversation_ids` method is required to preserve
the current performance characteristics. Today, `Storage` uses a specialized
private helper (`get_expiring_timestamp`) that reads `metadata.json` as raw
JSON and extracts **only** the `expires_at` field without deserializing the
full `Conversation` struct. If `remove_ephemeral_conversations` iterated
through `load_conversation_metadata` instead, it would fully deserialize
every conversation's metadata into memory just to check a timestamp —
a significant regression on workspaces with many conversations.

`FsStorageBackend` implements `load_expired_conversation_ids` by delegating
to this optimized reader. `InMemoryStorageBackend` checks its in-memory
`Conversation` structs directly, which is equally fast.

### Filesystem Path Methods

`Workspace` currently exposes several methods that return filesystem paths
for individual conversations:

- `build_conversation_dir(id, title, user)` — construct the expected dir path
- `conversation_dir(id)` — find an existing dir on disk
- `conversation_events_path(id)` — path to `events.json`
- `conversation_metadata_path(id)` — path to `metadata.json`
- `conversation_base_config_path(id)` — path to `base_config.json`

These are used by the CLI for editor integration (`jp conversation edit`),
the `jp conversation path` command, config loading (reading
`base_config.json`), and plugin dispatch (which needs `storage_path` and
`user_storage_path`). They are inherently filesystem-specific and do not
belong on `Workspace` or the backend traits.

These methods are removed from `Workspace` entirely. The CLI retains a
typed `Arc<FsStorageBackend>` reference in its `Ctx` (context) struct and
calls the path methods on the backend directly:

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

CLI commands that need filesystem paths access `ctx.fs_backend` instead of
`ctx.workspace`. For example, `jp conversation path` calls
`ctx.fs_backend.as_ref()?.conversation_dir(id)` instead of
`ctx.workspace.conversation_dir(id)`. Commands that require filesystem
paths (like `conversation edit` or `conversation path`) return a CLI-level
error when `fs_backend` is `None`.

The config loading pipeline (`load_partial_configs_from_files`) currently
takes `Option<&Workspace>` to read `storage_path()` and
`user_storage_path()`. This changes to accept an
`Option<&FsStorageBackend>` parameter instead.

This keeps `Workspace` free of infrastructure coupling — it depends only
on the four abstract traits. The filesystem-specific typed reference lives
in the imperative shell (`jp_cli`), which is the appropriate layer for
infrastructure concerns.

### Error Type Boundary

`PersistBackend` currently lives in `jp_workspace::persist` and its methods
return `jp_workspace::error::Result`. Moving the trait to `jp_storage`
(Phase 1) requires changing the error type to `jp_storage::error::Result`,
since `jp_storage` cannot depend on `jp_workspace`.

This is a straightforward migration: `jp_workspace::Error` already wraps
`jp_storage::Error` via `#[from]`, so callers in `jp_workspace` (e.g.,
`ConversationMut::flush`) can use `?` to convert automatically. The trait
methods in `jp_storage` use `jp_storage::error::Result`; the `Workspace`
methods that call them convert via the existing `From` impl.

### Construction Pattern

The current builder chain:

```rust
Workspace::new(root)
    .persisted_at(&storage)?     // creates Storage, sets self.storage
    .with_local_storage()?       // mutates Storage with user storage
```

is replaced by constructing the backend first, then passing it to
`Workspace`:

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

The fluent setter methods (`with_backend`, `with_persist`, `with_locker`, etc.)
keep construction ergonomic while making the backend selection explicit.
`FsStorageBackend::new` and `with_user_storage` encapsulate the current
`Storage` construction and user-storage setup. The CLI retains the typed
`Arc<FsStorageBackend>` for path queries and config loading, while `Workspace`
only sees the abstract traits.

## Drawbacks

**Four trait objects instead of one `Option<Storage>`.** The workspace now holds
four `Arc<dyn Trait>` where it previously held one `Option<Storage>`. This is
more indirection and more dynamic dispatch. In practice, the dispatch cost is
negligible — these methods are called at I/O boundaries, not in hot loops.

**Trait surface area.** Four traits with a combined ~18 methods is a meaningful
API surface to maintain. Adding a new storage operation requires updating the
trait, both implementations, and any mocks. However, this surface already
exists — it's just currently concrete methods on `Storage`. The trait makes the
contract explicit rather than implicit.

**`FsStorageBackend` wraps `Storage` rather than replacing it.** The internal
`Storage` struct survives as an implementation detail within `jp_storage`. This
is intentional — `Storage` contains filesystem logic (directory scanning,
symlink management, path resolution) that is genuinely filesystem-specific.
Flattening it into `FsStorageBackend` would just move the code without
simplifying it.

## Alternatives

### Single wide `StorageBackend` trait

One trait with all ~15 methods. Simpler to construct (one trait object), but
couples unrelated concerns. A mock that only needs to verify persistence must
still implement locking, loading, and session methods. The layered approach
lets tests mock only what they need.

### Enum dispatch instead of trait objects

```rust
enum StorageBackend { Fs(FsStorage), InMemory(InMemoryStorage) }
```

Avoids `dyn` overhead and `Arc` wrapping. But it's a closed set — adding a
new backend requires modifying the enum. Given the concrete plan for
browser/Web Storage backends, trait objects provide the needed extensibility.

### Keep `Option<Storage>` and widen `PersistBackend`

Incrementally add methods to `PersistBackend` until it covers all of
`Storage`'s functionality. The trait grows monotonically and `Option<Storage>`
is removed once the trait covers everything.

Rejected because the incremental approach leaves the codebase in a mixed state
for an extended period — some operations go through the trait, others go
through `Option<Storage>`. The layered approach achieves a clean state in one
step.

### Make `Storage` non-optional without traits

Remove the `Option` by requiring `Storage` in `Workspace::new`. Tests use
`tempdir`.

This eliminates the branching but doesn't solve the extensibility problem.
Non-filesystem backends (in-memory, browser, database) still can't be used.

## Non-Goals

- **Database or browser storage backends.** This RFD defines the traits and
  provides filesystem and in-memory implementations. Other backends are future
  work.

- **Project file abstraction.** Whether `Workspace::root` should be replaced
  by a backend-agnostic `ProjectFiles` abstraction is a separate concern,
  addressed in a subsequent RFD.

- **Config loading abstraction.** The config pipeline reads from multiple
  filesystem roots (user-global, workspace, user-workspace). Abstracting this
  is out of scope.

- **Changes to `Storage`'s internal logic.** The filesystem operations
  (directory scanning, conversation persistence, symlink management) remain
  as-is inside `Storage`. This RFD wraps them behind traits; it does not
  rewrite them.

## Risks and Open Questions

### Trait method granularity

The proposed traits group methods by concern (persist, load, lock, session).
An alternative grouping is by entity (conversation backend, session backend).
The concern-based split was chosen because it aligns with how the code
branches today, but implementation may reveal that a different cut is more
natural.

### Shared vs. separate backend instances

The design shows a single `FsStorageBackend` implementing all four traits.
An alternative is four separate structs, each implementing one trait. This
gives maximum flexibility (different persist and load backends) but adds
construction complexity. Starting with a single struct and splitting later if
needed is lower risk.

### Sanitization for non-filesystem backends

The current sanitization logic (trashing corrupt directories) is inherently
filesystem-specific. For in-memory backends, there is nothing to sanitize —
data is always structurally valid because it was written through typed APIs.
For future database backends, sanitization would look different (SQL
integrity checks, not directory scanning). The `LoadBackend::sanitize()`
method accommodates this by letting each backend define what "sanitize"
means. `InMemoryStorageBackend` returns an empty report;
`FsStorageBackend` runs the full directory scan and trash logic.

## Implementation Plan

### Phase 1: Define traits in `jp_storage`

Add the four trait definitions (`PersistBackend` is widened from the existing
trait in `jp_workspace::persist`, then moved to `jp_storage`; `LoadBackend`,
`LockBackend`, `SessionBackend` are new). Add `ConversationLockGuard` trait.
No implementations yet, no callers yet.

**Depends on:** Nothing.
**Mergeable:** Yes.

### Phase 2: `FsStorageBackend`

Implement all four traits for `FsStorageBackend`, delegating to the existing
`Storage` methods. This is a mechanical wrapping — no behavior changes.

**Depends on:** Phase 1.
**Mergeable:** Yes.

### Phase 3: `InMemoryStorageBackend`

Implement all four traits for `InMemoryStorageBackend` using in-memory data
structures. Add tests that exercise the same operations against both
backends to verify behavioral equivalence.

**Depends on:** Phase 1.
**Mergeable:** Yes (can be done in parallel with Phase 2).

### Phase 4: Refactor `Workspace`

Replace `Option<Storage>` with the four trait objects. Replace the
`disable_persistence` boolean with a backend-swap implementation (the
method signature is unchanged). Update all methods that branched on
`self.storage.as_ref()` to call through the
traits. Update `ConversationLock` and `ConversationMut` to hold
non-optional writer and lock guard.

Update the CLI's `--no-persist` path to construct the workspace with
`NullPersistBackend` instead of calling `disable_persistence()` after
construction. The error-path call to `disable_persistence()` remains
unchanged — it now swaps the backend instead of flipping a boolean.

This is the large phase — it touches `Workspace`, `ConversationLock`,
`ConversationMut`, and all their callers.

**Depends on:** Phase 2 and Phase 3.
**Mergeable:** Yes, but this is a single large PR.

### Phase 5: Migrate tests

Replace test code that creates `Workspace::new(Utf8PathBuf::new())` (no
storage) with `Workspace::new(id, &InMemoryStorageBackend::default())`.
Replace tests that create tempdir-backed workspaces and call
`disable_persistence()` with `InMemoryStorageBackend`.

**Depends on:** Phase 4.
**Mergeable:** Yes.

### Phase 6: Remove dead code

Remove `MockPersistBackend` from `jp_workspace::persist` (replaced by
`InMemoryStorageBackend`). Remove `Workspace::new()` constructor that
takes only a root path (replaced by constructor that takes backends).
Clean up `#[cfg(debug_assertions)]` test helpers that were workarounds
for the optional storage model. Remove the `disable_persistence` boolean
field from `Workspace` (the method remains, backed by the trait swap).

**Depends on:** Phase 5.
**Mergeable:** Yes.

## References

- [RFD 020] — Parallel Conversations. Introduced conversation locking and
  session-based tracking.
- [RFD 031] — Durable Conversation Storage with Workspace Projection.
  Redesigns the storage model around user-local as source of truth.
- [RFD 052] — Workspace Data Store Sanitization. Defines `Workspace::sanitize()`
  which this RFD restructures.
- [RFD 069] — Guard-Scoped Persistence for Conversations. Introduced
  `PersistBackend` and `ConversationMut` auto-persist, which this RFD builds on.

[RFD 020]: 020-parallel-conversations.md
[RFD 031]: 031-durable-conversation-storage-with-workspace-projection.md
[RFD 052]: 052-workspace-data-store-sanitization.md
[RFD 069]: 069-guard-scoped-persistence-for-conversations.md
