# RFD D36: Live Workspace View for Long-Running Plugin Hosts

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-08

## Summary

Long-running plugin hosts such as `jp serve` return stale conversation data
because `Workspace`'s read methods cache aggressively for `jp query`'s
read-your-writes semantics, and the cache is never invalidated within the
host's lifetime. This RFD introduces `LiveWorkspace<'a>`, a `&mut`-borrowed
view over `Workspace` that provides method-level freshness: every read
revalidates the index and force-reloads the relevant cell from disk before
returning. The plugin runner is plumbed with `&mut LiveWorkspace<'_>`
end-to-end, making the cached `Workspace::events()` etc. structurally
unreachable from plugin handlers.

## Motivation

The `jp-serve-web` plugin from [PR #546] shows stale conversations to the
browser even after refresh. The user runs `jp query` in another terminal to
add a turn, refreshes the web page, and the new turn is invisible until
`jp serve` is restarted. The bug reproduces with any sequence of disk-changing
operations from a separate `jp` process.

The cause is in the host, not the plugin. Plugin handlers ask the host fresh
on every protocol message — `PluginToHost::ListConversations` and
`PluginToHost::ReadEvents`. The host's dispatch handlers in
`crates/jp_cli/src/cmd/plugin/dispatch.rs` answer them by calling
`workspace.conversations()` and `workspace.events(&handle)`, both of which
read from `OnceLock<Arc<RwLock<...>>>` cells in `state::State`. These cells
are populated on first access and never re-read from disk.

The cache is load-bearing for `jp query`. The query loop reads events many
times per turn, and reads need to see in-memory writes from `ConversationMut`
before they are flushed to disk (see [RFD 069]). The cache is the in-process
synchronization mechanism for read-your-writes; we cannot simply remove it.

The deeper architectural fact: `Workspace` serves two roles. As a mutable
session for `jp query`, it provides read-your-writes through its cache. As a
read API for any caller, it provides whatever was on disk at first access.
These roles want different freshness semantics, and the current API conflates
them. Any long-running plugin reader — not just `jp-serve-web`, but a future
TUI dashboard, metrics collector, or chat interface — hits the same bug under
the current shape.

## Design

### Goals

- **Plugin handlers cannot return stale data.** Type-system enforced; no
  per-call discipline.
- **`jp query`'s hot path is unchanged.** Cache, read-your-writes, and lock
  semantics are all preserved exactly.
- **API parity.** Plugin host code uses the same `acquire_conversation` →
  handle → read pattern as built-in commands.
- **No new traits, no async runtime, no platform-specific code.** Correctness
  fix without committing to file-watcher infrastructure.

### `LiveWorkspace<'a>`

A view type that borrows `&mut Workspace` and exposes the same
conversation-reading API as `Workspace`, but with method-level freshness:

```rust
pub struct LiveWorkspace<'a> {
    workspace: &'a mut Workspace,
}

impl Workspace {
    pub fn live(&mut self) -> LiveWorkspace<'_> {
        LiveWorkspace { workspace: self }
    }
}

impl LiveWorkspace<'_> {
    pub fn acquire_conversation(
        &mut self,
        id: &ConversationId,
    ) -> Result<ConversationHandle>;

    pub fn conversations(
        &mut self,
    ) -> Result<Vec<(ConversationId, ArcRwLockReadGuard<RawRwLock, Conversation>)>>;

    pub fn metadata(
        &mut self,
        handle: &ConversationHandle,
    ) -> Result<RwLockReadGuard<'_, Conversation>>;

    pub fn events(
        &mut self,
        handle: &ConversationHandle,
    ) -> Result<RwLockReadGuard<'_, ConversationStream>>;

    pub fn lock_conversation(
        &mut self,
        handle: ConversationHandle,
        session: Option<&Session>,
    ) -> Result<LockResult>;
}
```

The handle type is unchanged. `ConversationHandle` proves "this id was known
to the index at acquire time"; freshness is the receiver's responsibility:

- `Workspace::events(handle)` returns cached data (today's behavior).
- `LiveWorkspace::events(handle)` revalidates the index and force-reloads the
  cell from disk, then returns through the same `RwLockReadGuard` shape.

Same handle, same return type, different freshness semantics by virtue of
which type's method you call.

### Shared private helpers on `Workspace`

```rust
impl Workspace {
    /// Reconcile the in-memory index against disk by **diff**, not reset.
    ///
    /// Scan IDs from storage, then:
    /// - insert empty cells for IDs not yet known,
    /// - remove cells for IDs no longer present,
    /// - preserve existing cells (and their `Arc<RwLock<_>>` identity) for
    ///   IDs that still exist.
    ///
    /// This is distinct from `load_conversation_index`, which replaces
    /// `State` wholesale and is only safe at workspace construction. Calling
    /// the reset-style loader on a workspace with active `ConversationLock`
    /// or `ConversationMut` instances would detach their `Arc`s from the
    /// workspace cache.
    fn refresh_index(&mut self) -> Result<()>;

    /// Replace the contents of an existing cell pair with fresh data from
    /// the loader. The `Arc<RwLock<_>>` identity is preserved, so anyone
    /// holding the `Arc` sees the new contents through their existing
    /// reference.
    ///
    /// Atomicity: loads both metadata and events from the loader **before**
    /// writing either cell. A load failure leaves both cells unchanged.
    /// This avoids leaving the cell pair in a half-refreshed state where,
    /// e.g., metadata reflects disk but events still reflect the old
    /// snapshot.
    ///
    /// If a cell is uninitialized (not yet loaded), this initializes it
    /// with the fresh data. If the ID is not in the refreshed index,
    /// returns a clean `not_found` error.
    fn force_reload_conversation(&mut self, id: &ConversationId) -> Result<()>;

    /// Acquire the cross-process flock without populating cells. Used by
    /// the live view's lock_conversation, which sequences lock-then-reload
    /// rather than the cached path's lazy-init-then-lock.
    fn try_lock_without_reload(
        &self,
        handle: ConversationHandle,
        session: Option<&Session>,
    ) -> Result<LockResult>;
}
```

`LiveWorkspace` methods compose them:

```rust
impl LiveWorkspace<'_> {
    pub fn events(
        &mut self,
        handle: &ConversationHandle,
    ) -> Result<RwLockReadGuard<'_, ConversationStream>> {
        self.workspace.refresh_index()?;
        self.workspace.force_reload_conversation(handle.id())?;
        self.workspace.events(handle)
    }
}
```

The `Workspace::events` call after `force_reload_conversation` returns a
guard whose contents were freshly loaded from disk. Same `Arc<RwLock<_>>`,
replaced contents.

### `lock_conversation` flow

The locking path is the load-bearing piece for the future chat-UI write path.
It preserves `jp query`-style read-your-writes inside the locked operation by
revalidating *while the flock is held*:

```
1. refresh_index()
2. acquire_conversation(id) — verify the id is in the refreshed index
3. try_lock_without_reload() — get the cross-process flock
   if not acquired: return AlreadyLocked(handle)
4. construct ConversationLock from the existing Arc<RwLock<_>> cells
5. force_reload_conversation(id) — replace cell contents from disk
   if this fails: drop the lock, propagate the error
6. return the ConversationLock
```

Steps 3 and 5 are atomic with respect to other processes (we hold the flock).
Steps 1–5 are serial within this process. Constructing the lock at step 4
before the force-reload at step 5 is intentional and safe: the lock holds the
same `Arc<RwLock<_>>` instances that step 5 mutates through, so by the time
the lock is returned at step 6 its cells reflect disk at the moment of
acquisition. Any subsequent `Workspace::events()` call by code holding the
lock — including a future chat-UI write handler running its query loop
through `ConversationMut` — sees the fresh state and behaves identically to
today's `jp query` flow.

If step 5 fails, the lock constructed in step 4 is dropped (releasing the
flock cleanly) and the loader error propagates to the caller. No
partially-handed-out lock; no panic.

### Plugin runner integration

`run_plugin` constructs the live view at entry and passes it through:

```rust
pub(crate) fn run_plugin(
    // ...
    workspace: &mut Workspace,
    // ...
) -> Result<(), cmd::Error> {
    let mut live = workspace.live();
    message_loop(reader, &stdin, &mut live, &config_json, &shutdown_sent)
}

fn message_loop(
    reader: BufReader<impl std::io::Read>,
    stdin: &Mutex<impl Write>,
    workspace: &mut LiveWorkspace<'_>,
    // ...
) -> Result<(), cmd::Error> { ... }

fn handle_read_events(
    workspace: &mut LiveWorkspace<'_>,
    conversation_id: &str,
    req_id: Option<String>,
) -> HostToPlugin { ... }
```

Plugin handlers receive `&mut LiveWorkspace<'_>` only. `&Workspace` is not in
scope anywhere in plugin-host code. New protocol message handlers added later
inherit the freshness guarantee structurally; no per-handler discipline is
required.

### No persistent cache field

`LiveWorkspace` does not hold an internal request cache. Each method call
revalidates the index and force-reloads the relevant cell.

If repeated reads inside one IPC operation become measurably expensive, an
opt-in scoped cache can be added later as a closure-based wrapper:

```rust
live.with_operation_cache(|live| handle_message(msg, live))
```

The closure scope bounds the cache lifetime. **Forgetting `with_operation_cache`
makes a handler do extra disk reads but does not return stale data.** The
cache is purely a performance optimization; correctness must not depend on it.

For v1, even the closure version is YAGNI. Personal-workspace `events.json`
reloads are sub-millisecond. The cache lands when measurement shows it pays
for itself.

## Drawbacks

- **One new public type and one new public method on `Workspace`.** Modest
  API surface growth. The type is small (5 methods); the method is a
  one-liner.

- **Per-call disk reads.** Every `LiveWorkspace::events()` call reads
  `events.json` from disk and parses it. At personal-workspace scale this is
  sub-millisecond per file. For workspaces with very large conversations or
  very high request rates, the cost grows; the deferred closure-scoped cache
  is the response.

- **`&mut LiveWorkspace<'_>` serializes plugin host operations.** The dispatch
  loop processes messages serially today, so this is not a current
  constraint. If the host ever moves to concurrent message handling, the
  `&mut` borrow becomes a bottleneck — but the same redesign would be
  required regardless of how freshness is implemented (the dirty-state
  hazard described under [Risks](#risks-and-open-questions) only arises
  under concurrent handling).

## Alternatives

### `WorkspaceReader` (read-only sibling type)

A separate type holding `Arc<dyn LoadBackend>` directly, read-only API, used
by plugin host code instead of `Workspace`.

Rejected because the type becomes useless (or requires an awkward escape
hatch back to `Workspace`) when chat UI introduces write operations through
the same plugin host. `LiveWorkspace<'a>` borrows the workspace, so it can
expose the locking path naturally.

### Mtime-on-read auto-invalidation

Cache cells gain a `loaded_mtime` field; each `Workspace::events()` call does
a `stat()` and reloads if the file is newer. Single API, single type, no
caller discipline.

Rejected because every read on the `jp query` hot path pays the `stat()`
cost (~1–5 µs per call) for no benefit — `jp query` is the only writer of
its conversation, so its cache is always trustworthy. The architectural seam
is wrong: freshness shouldn't be a per-read property of the cache; it should
be a property of the caller's role.

### File-watcher push notifications (`notify` crate)

A background task watches the storage tree and invalidates cells on
filesystem events. Cache becomes self-maintaining; reads are cache hits.

Strictly more powerful than `LiveWorkspace`, since the same event source
could later drive SSE-style live updates without browser refresh. Rejected
for v1 because:

- Materially more implementation: new `WatchBackend` trait, `notify`
  dependency, tokio task lifecycle, debouncing, error/restart handling.
- Platform-specific quirks: inotify watch limits on Linux, FSEvents
  coalescing on macOS, no events on NFS.
- Coordination with `ConversationMut` requires a dirty-flag mechanism on
  cells.

The watcher is the right answer if/when [RFD D16]'s "no live updates"
Non-Goal becomes a real requirement (real-time updates without browser
refresh). It composes orthogonally with `LiveWorkspace`: the watcher would
drive `force_reload_conversation` from outside the request path. Building it
now is YAGNI for the in-scope problem.

### Polling background worker

A scheduled task (~1 Hz) reloads cells periodically. Single API, no `notify`
dependency, eventually-consistent.

Rejected because polling is a probabilistic fix to a synchronous correctness
problem — a browser refresh may or may not see fresh data depending on
poll-cycle timing. Less predictable than today's "always stale until
restart." The watcher (above) is the push-based version of the same idea
and is strictly better; if push-based reload is wanted, build the watcher.

### `reload_*` methods on `Workspace`

Add `reload_metadata`/`reload_events`/`reload_index` on `Workspace`; callers
opt in by calling them before reading.

Rejected because correctness depends on every call site remembering to call
`reload_*`. New plugin handlers added later have no compile-time signal that
they need to opt in. The hazard is exactly the bug class this RFD aims to
eliminate.

## Non-Goals

- **Real-time live updates.** The browser still has to refresh to see new
  data. SSE / WebSocket-driven updates are deferred to a future RFD that
  introduces a watcher or push channel.

- **Dirty-state tracking on cells.** Today's dispatch loop processes protocol
  messages serially (one stdin line at a time), so a `force_reload_conversation`
  call cannot race with a live `ConversationMut` in the same host process.
  If we ever introduce concurrent message handling in the host, or expose
  plugin-side `lock`/`unlock` as separate protocol messages so a plugin can
  hold a lock across IPC boundaries, the dirty-state hazard becomes real and
  we need to add tracking. Until then, deferred.

- **Changing `jp query`'s behavior.** `Workspace`'s public API is unchanged;
  the query loop is untouched.

- **Fixing every long-running scenario.** This RFD covers plugin hosts.
  Long-running CLI commands that read conversation data without going through
  the plugin protocol are not affected (none currently exist).

- **The chat-UI write path itself.** This RFD makes the plugin host's write
  path *possible* via `LiveWorkspace::lock_conversation`, but the chat-UI
  feature itself is out of scope.

## Risks and Open Questions

- **Naming.** `LiveWorkspace` reads as "real-time / push-based" in a UI
  context, which is the opposite of what this type does. Alternatives
  considered: `WorkspaceSession`, `FreshWorkspace`, `RevalidatingWorkspace`.
  Bikeshed; pick a name that doesn't promise features the type doesn't have.

- **Dirty-state invariant in rustdoc.** `force_reload_conversation` assumes
  no live `ConversationMut` exists for the same conversation in this
  process. Document this as a `# Invariants` section on `LiveWorkspace`'s
  rustdoc and as `// Invariant:` comments on the helper (not `// SAFETY:`,
  which is reserved for `unsafe` code by Rust convention). The invariants
  to spell out:

  - Plugin message handling is serial; one handler runs at a time.
  - Plugin-held locks are not exposed across IPC messages.
  - Within a single handler, callers must not invoke `LiveWorkspace`
    revalidating reads for a conversation while holding a dirty
    `ConversationMut` for that same conversation. Once a handler holds a
    `ConversationLock`, reads must go through `ConversationLock` or
    `ConversationMut`, not through `LiveWorkspace`.
  - If the runner ever moves to concurrent message handling, or exposes
    explicit plugin-side `lock`/`unlock` protocol messages that span IPC
    boundaries, these invariants are broken and the design must be
    revisited (likely by adding dirty-state tracking on cells).

  The serial loop covers the cross-message hazard automatically. The
  same-handler hazard is not enforced by Rust's borrow checker (a handler
  could hold a `ConversationLock` and still call `live.events(handle)`),
  so the rustdoc has to state the rule explicitly.

- **Race: conversation deleted between refresh and lock.**
  `LiveWorkspace::lock_conversation`'s sequence (refresh → acquire → flock
  → force_reload) can fail at step 4 if another process removes the
  conversation directory between steps 1 and 4. The error propagates as
  `not_found`. Caller handles it. Worth a regression test.

- **Lifetime ergonomics.** `LiveWorkspace<'a>` exclusively borrows
  `&'a mut Workspace`. Code holding `LiveWorkspace` cannot also access
  `Workspace` directly during that scope. For the plugin runner this is the
  desired behavior. For any future caller that wants to switch back and
  forth, they will need to drop `LiveWorkspace` and re-call
  `Workspace::live()` — cheap (one struct construction), but worth noting
  in the rustdoc.

## Implementation Plan

### Phase 1: Implementation

- Add private helpers to `Workspace`: `refresh_index`,
  `force_reload_conversation`, `try_lock_without_reload`. Refactor existing
  `load_conversation_index` to share `refresh_index`'s logic where
  appropriate (preserve existing cells, diff against disk).
- Add `LiveWorkspace<'a>` and `Workspace::live(&mut self)`.
- Implement `LiveWorkspace::acquire_conversation`, `conversations`,
  `metadata`, `events`, `lock_conversation`.
- Update `crates/jp_cli/src/cmd/plugin/dispatch.rs`:
  - `run_plugin` calls `workspace.live()` and passes
    `&mut LiveWorkspace<'_>` to `message_loop`.
  - `message_loop` and dispatch handlers (`handle_list_conversations`,
    `handle_read_events`) take `&mut LiveWorkspace<'_>`.

### Phase 2: Tests

Six regression tests, each running two `jp` host processes against a shared
workspace unless noted otherwise:

1. **Creation drift.** Writer creates a new conversation; reader sees it via
   `LiveWorkspace::conversations()` on the next protocol message, and can
   `acquire_conversation` it.
2. **Deletion drift.** Writer removes a conversation; reader's next
   `LiveWorkspace::conversations()` no longer lists it. (This test fails if
   `refresh_index` only adds IDs without removing vanished ones.)
3. **Stale handle after deletion.** Reader holds a `ConversationHandle`
   acquired before deletion; after the writer removes the conversation, the
   reader's next `LiveWorkspace::events(&handle)` returns a clean `not_found`
   error rather than serving stale cached data.
4. **Event drift.** Writer appends a turn to an existing conversation;
   reader sees the new turn via `LiveWorkspace::events(handle)` on the next
   protocol message.
5. **Lock force-reload.** Single-process test. Writer-A holds a lock,
   appends events, releases. Reader (same process, separate `LiveWorkspace`
   scope) acquires a lock via `LiveWorkspace::lock_conversation` and verifies
   the resulting `ConversationLock::events()` reflects the appended turn,
   confirming step 5 of the lock flow ran.
6. **Delete-during-lock-acquisition race.** Writer removes a conversation
   between the reader's `refresh_index` and `force_reload_conversation`;
   reader gets a clean `not_found` error and the flock is released, not a
   panic or a partially-handed-out lock.

### Phase 3 (deferred): operation-scoped cache

If measurements show redundant in-handler disk reads becoming expensive,
add the closure-scoped `with_operation_cache`. Not part of v1.

## References

- [PR #546] — `jp-serve-web` plugin (the reporting context).
- [RFD 069] — Guard-scoped persistence; defines the
  `OnceLock<Arc<RwLock<_>>>` cell structure that this RFD reaches into via
  `force_reload_conversation`.
- [RFD 072] — Command plugin system; defines the JSON-lines protocol that
  `jp-serve-web` uses.
- [RFD 073] — Layered storage backend; defines the `LoadBackend` trait that
  `force_reload_conversation` calls through.
- [RFD D16] — Original read-only web UI draft (predates the plugin
  extraction in [PR #546]).

[PR #546]: https://github.com/dcdpr/jp/pull/546
[RFD 069]: 069-guard-scoped-persistence-for-conversations.md
[RFD 072]: 072-command-plugin-system.md
[RFD 073]: 073-layered-storage-backend-for-workspaces.md
[RFD D16]: drafts/D16-read-only-web-ui-for-conversations.md
