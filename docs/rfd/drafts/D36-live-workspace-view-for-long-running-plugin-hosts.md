# RFD D36: Live Workspace View for Long-Running Plugin Hosts

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-08
- **Summary**: Introduces a `LiveWorkspace` view that revalidates the workspace
  index and reloads conversations from disk on every plugin read.

## Summary

Long-running plugin hosts such as `jp serve` return stale conversation data
because `Workspace`'s read methods cache state in memory for `jp query`'s
read-your-writes semantics.
This RFD introduces `LiveWorkspace<'a>`, a `&mut`-borrowed view over `Workspace`
whose read methods revalidate the conversation index and reload the target
conversation from disk on every call.
Plumbing `&mut LiveWorkspace<'_>` through the plugin runner makes cached
`Workspace` read methods unreachable from plugin request handlers.

## Motivation

The `jp-serve-web` plugin from [PR #546] shows stale conversations in the
browser even after a refresh.
When a user runs `jp query` in another terminal to add a turn and refreshes the
page, the new turn remains invisible until `jp serve` restarts.
Any disk write from a separate `jp` process triggers the same bug.

The cause is in the host, not the plugin.
The plugin requests fresh data from the host on every
`PluginToHost::ListConversations` and `PluginToHost::ReadEvents` message.
The dispatch handlers in `crates/jp_cli/src/cmd/plugin/dispatch.rs` serve those
requests by calling `workspace.conversations()` and `workspace.events(&handle)`.
Both methods read from `OnceLock<Arc<RwLock<...>>>` cells in `state::State` that
are populated on first access and never reloaded from disk.

`jp query` depends on this cache.
The query loop reads events many times per turn and must see in-memory writes
from `ConversationMut` before they are flushed to disk (see [RFD 069]).
Because the cache provides this in-process read-your-writes synchronization,
removing it is not an option.

`Workspace` conflates two roles that require different freshness semantics.
As a mutable session for `jp query`, it serves cached in-memory state for
read-your-writes.
As a read API for other callers, it serves whatever was on disk at first access.
Any long-running plugin reader (such as `jp-serve-web`, or a future TUI
dashboard, metrics collector, or chat interface) hits the same staleness bug.

## Design

### Goals

- **Plugin handlers cannot return stale data.** Enforce freshness in the type
  system rather than relying on per-call discipline.
- **`jp query`'s hot path is unchanged.** Preserve existing cache,
  read-your-writes, and lock semantics.
- **API parity.** Keep the `acquire_conversation` → handle → read pattern used
  by built-in commands.
- **No new traits, async runtime, or platform-specific code.** Fix the
  correctness bug without introducing file-watcher infrastructure.

### `LiveWorkspace<'a>`

`LiveWorkspace<'a>` borrows `&mut Workspace` and exposes the same
conversation-reading API as `Workspace`, but revalidates on-disk state on each
call:

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

`ConversationHandle` is unchanged.
It proves only that the conversation ID existed in the index when acquired;
freshness depends on the receiver:

- `Workspace::events(handle)` returns the cached stream.
- `LiveWorkspace::events(handle)` revalidates the index, reloads the cell from
  disk, and returns the same `RwLockReadGuard` type.

### Shared private helpers on `Workspace`

`Workspace` provides three private helpers for index reconciliation, cell
reloading, and lock acquisition:

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

`force_reload_conversation` replaces the contents of the existing
`Arc<RwLock<_>>` in place, so the subsequent `Workspace::events` call returns a
guard over the freshly loaded data.

### `lock_conversation` flow

`LiveWorkspace::lock_conversation` supports a future chat-UI write path.
It preserves `jp query`'s read-your-writes semantics inside the locked operation
by reloading from disk while holding the cross-process `flock`:

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

Steps 1 through 5 execute serially within this process, and steps 3 through 5
are atomic across processes because the `flock` is held.
Constructing `ConversationLock` in step 4 before reloading in step 5 is safe
because the lock shares the `Arc<RwLock<_>>` cells that
`force_reload_conversation` updates in place.
When step 6 returns the lock, its cells hold the on-disk state as of lock
acquisition.
Any subsequent `Workspace::events()` call while the lock is held, such as a
chat-UI write handler running a query loop through `ConversationMut`, reads that
fresh state and sees its own writes just as `jp query` does.

If step 5 fails, dropping the lock from step 4 releases the `flock` and
propagates the loader error to the caller without panicking or returning a
partially initialized lock.

### Plugin runner integration

`run_plugin` constructs `LiveWorkspace` at entry and threads it through the
dispatch loop:

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

Plugin handlers receive only `&mut LiveWorkspace<'_>`; `&Workspace` is never in
scope in plugin-host code.
New protocol message handlers inherit this freshness guarantee automatically.

### No internal request cache

`LiveWorkspace` does not hold an internal request cache.
Each method call revalidates the index and reloads the target cell.

If repeated reads inside one IPC operation become measurably expensive, an
opt-in scoped cache can be added later as a closure-based wrapper:

```rust
live.with_operation_cache(|live| handle_message(msg, live))
```

The closure bounds the cache lifetime to a single operation.
Omitting `with_operation_cache` causes extra disk reads but never returns stale
data, keeping caching a pure performance optimization.

Reloading `events.json` in a personal workspace takes under a millisecond, so
`v1` omits even the closure-scoped cache until profiling justifies it.

## Drawbacks

- **Larger `Workspace` API surface.** Adds `LiveWorkspace` (five methods) and
  the one-line `Workspace::live` constructor.

- **Per-call disk reads.** Every `LiveWorkspace::events()` call reads and parses
  `events.json` from disk.
  This takes under a millisecond per file in personal workspaces, but the cost
  grows with large conversations or high request rates.
  The deferred `with_operation_cache` wrapper addresses this if needed.

- **`&mut LiveWorkspace<'_>` serializes plugin host operations.** The dispatch
  loop already processes messages serially, so the exclusive borrow imposes no
  extra constraint.
  Supporting concurrent message handling would require redesigning
  synchronization regardless, since concurrent access also introduces the
  dirty-state hazard described in [Risks and Open
  Questions](#risks-and-open-questions).

## Alternatives

### `WorkspaceReader` (read-only sibling type)

Introduce a separate read-only type that holds `Arc<dyn LoadBackend>` directly
and replaces `Workspace` in the plugin host.

Rejected because a read-only type cannot support write operations when the chat
UI adds them to the plugin host, without an awkward escape hatch back to
`Workspace`.
Because `LiveWorkspace<'a>` borrows `Workspace`, it can expose
`lock_conversation` directly.

### Mtime-on-read auto-invalidation

Add a `loaded_mtime` field to each cache cell so `Workspace::events()` runs
`stat()` on every call and reloads when the file on disk is newer.
This keeps a single `Workspace` type and requires no caller changes.

Rejected because every read on the `jp query` hot path would pay the `stat()`
cost (roughly 1 to 5 µs per call) for no benefit: `jp query` is the sole writer
of its conversation, so its in-memory cache is already authoritative.
Freshness is a property of the caller's role, not of individual cache reads.

### File-watcher push notifications (`notify` crate)

Run a background task that watches the storage directory and invalidates cache
cells on filesystem events, keeping reads as in-memory cache hits.

A file watcher is more capable than `LiveWorkspace` because the same event
stream could drive Server-Sent Events (SSE) for live browser updates without
page refreshes.
It is rejected for `v1` because it requires:

- A new `WatchBackend` trait, the `notify` dependency, a background task
  lifecycle, event debouncing, and error recovery.
- Handling platform-specific behavior: `inotify` watch limits on Linux, FSEvents
  coalescing on macOS, and missing events on network filesystems (NFS).
- Dirty-state tracking on cache cells to coordinate with active
  `ConversationMut` guards.

A watcher is the right choice if real-time browser updates (explicitly deferred
in [RFD D16]) become a requirement, and it composes cleanly with this design by
calling `force_reload_conversation` when filesystem events arrive.

### Polling background worker

Run a background task (for example, at 1 Hz) that periodically reloads cache
cells.
This preserves a single `Workspace` API without adding a `notify` dependency.

Rejected because periodic polling cannot guarantee synchronous freshness:
whether a browser refresh sees new data depends on poll timing, which is less
predictable than deterministic staleness.
If background invalidation is needed, a filesystem watcher is strictly better
than polling.

### `reload_*` methods on `Workspace`

Add explicit `reload_metadata`, `reload_events`, and `reload_index` methods to
`Workspace` for callers to invoke before reading.

Rejected because correctness would depend on every call site remembering to
reload first.
Future plugin handlers would receive no compile-time error if they called
`workspace.events()` without reloading.

## Non-Goals

- **Real-time live updates.** The browser still requires a refresh to show new
  data.
  Push updates over SSE or WebSockets are deferred to a future RFD.

- **Dirty-state tracking on cells.** The dispatch loop processes protocol
  messages serially (one `stdin` line at a time), so `force_reload_conversation`
  cannot race with an active `ConversationMut` in the same host process.
  Dirty-state tracking is only needed if the host adopts concurrent message
  handling or exposes separate `lock` and `unlock` protocol messages that let
  plugins hold locks across IPC boundaries.

- **Changing `jp query` behavior.** `Workspace`'s existing public methods and
  the query loop remain unchanged.

- **Non-plugin long-running commands.** This RFD scopes revalidation to plugin
  hosts.
  No built-in long-running CLI commands currently read conversation data outside
  the plugin protocol.

- **The chat-UI write path.** `LiveWorkspace::lock_conversation` enables writes
  from plugin hosts, but implementing the chat UI's write handlers is out of
  scope.

## Risks and Open Questions

- **Naming.** In a UI context, `LiveWorkspace` can suggest real-time push
  updates rather than on-demand revalidation.
  Candidate names include `WorkspaceSession`, `FreshWorkspace`, and
  `RevalidatingWorkspace`.

- **Dirty-state invariant in rustdoc.** `force_reload_conversation` assumes no
  active `ConversationMut` exists for the target conversation in the same
  process.
  Document this under `# Invariants` in `LiveWorkspace`'s rustdoc and in `//
  Invariant:` comments on the helper (`// SAFETY:` is reserved for `unsafe`
  code):

  - Plugin message handling is serial, running one handler at a time.
  - Plugin-held locks cannot span IPC messages.
  - Within a single handler, callers must not call `LiveWorkspace` read methods
    on a conversation while holding a dirty `ConversationMut` for it.
    Once a handler acquires a `ConversationLock`, reads must go through
    `ConversationLock` or `ConversationMut`.
  - Concurrent message handling or cross-message `lock`/`unlock` requests would
    violate these invariants and require dirty-state tracking on cache cells.

  Serial dispatch prevents cross-message races automatically, but Rust's borrow
  checker does not prevent a single handler from holding a `ConversationLock`
  and still calling `live.events(handle)`.
  The rustdoc must state that restriction explicitly.

- **Conversation deleted between index refresh and reload.** In
  `LiveWorkspace::lock_conversation` (`refresh_index` → `acquire_conversation`
  → `try_lock_without_reload` → `force_reload_conversation`), another process
  can delete the conversation directory after `refresh_index` succeeds but
  before `force_reload_conversation` reads from disk.
  In that case, `force_reload_conversation` returns a `not_found` error to the
  caller after releasing the lock; Phase 2 includes a regression test for this
  race.

- **Borrow scope ergonomics.** Because `LiveWorkspace<'a>` holds `&'a mut
  Workspace`, callers cannot access `Workspace` directly while `LiveWorkspace`
  is alive.
  That exclusivity is intentional in the plugin runner; callers that need to
  alternate between cached and revalidating access can drop `LiveWorkspace` and
  call `Workspace::live()` again, which only constructs a single-pointer
  wrapper.

## Implementation Plan

### Phase 1: Implementation

- Add private helpers `refresh_index`, `force_reload_conversation`, and
  `try_lock_without_reload` to `Workspace`.
  Refactor `load_conversation_index` to share diff-based reconciliation logic
  with `refresh_index`.
- Add `LiveWorkspace<'a>` and `Workspace::live(&mut self)`.
- Implement `acquire_conversation`, `conversations`, `metadata`, `events`, and
  `lock_conversation` on `LiveWorkspace`.
- Update `crates/jp_cli/src/cmd/plugin/dispatch.rs`:
  - `run_plugin` calls `workspace.live()` and passes `&mut LiveWorkspace<'_>` to
    `message_loop`.
  - `message_loop` and the dispatch handlers (`handle_list_conversations` and
    `handle_read_events`) take `&mut LiveWorkspace<'_>`.

### Phase 2: Tests

Add six regression tests, each running two `jp` host processes against a shared
workspace unless noted:

1. **Creation drift.** The writer creates a conversation; on the next protocol
   message, the reader sees it via `LiveWorkspace::conversations()` and can pass
   its ID to `acquire_conversation`.
2. **Deletion drift.** The writer removes a conversation; the reader's next
   `LiveWorkspace::conversations()` call omits it, verifying that
   `refresh_index` removes deleted IDs as well as inserting new ones.
3. **Stale handle after deletion.** The reader acquires a `ConversationHandle`
   before the writer removes the conversation; the reader's next
   `LiveWorkspace::events(&handle)` call returns `not_found` instead of cached
   data.
4. **Event drift.** The writer appends a turn to an existing conversation; the
   reader sees the new turn via `LiveWorkspace::events(handle)` on the next
   protocol message.
5. **Lock reload.** In a single process, writer A locks a conversation, appends
   events, and releases the lock.
   A reader in a separate `LiveWorkspace` scope calls
   `LiveWorkspace::lock_conversation` and verifies that
   `ConversationLock::events()` includes the appended turn, confirming step 5 of
   the lock sequence.
6. **Deletion during lock acquisition.** The writer removes a conversation
   between the reader's `refresh_index` and `force_reload_conversation` calls;
   the reader receives a `not_found` error and releases the `flock` cleanly.

### Phase 3: Operation-scoped cache (deferred)

If profiling shows that repeated disk reads within a single handler are
expensive, add the closure-scoped `with_operation_cache` helper.
Deferred from `v1`.

## References

- [PR #546]: `jp-serve-web` plugin where the staleness bug was reported.
- [RFD 069]: Guard-scoped persistence; defines the `OnceLock<Arc<RwLock<_>>>`
  cell structure updated by `force_reload_conversation`.
- [RFD 072]: Command plugin system; defines the JSON-lines protocol used by
  `jp-serve-web`.
- [RFD 073]: Layered storage backend; defines the `LoadBackend` trait called by
  `force_reload_conversation`.
- [RFD D16]: Original read-only web UI draft, written before plugin extraction
  in [PR #546].

[PR #546]: https://github.com/dcdpr/jp/pull/546
[RFD 069]: ../069-guard-scoped-persistence-for-conversations.md
[RFD 072]: ../072-command-plugin-system.md
[RFD 073]: ../073-layered-storage-backend-for-workspaces.md
[RFD D16]: D16-read-only-web-ui-for-conversations.md
