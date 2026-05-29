# RFD 074: Eager Loading with Command-Declared Data Requirements

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-15
- **Requires**: [RFD 073], [RFD 069]

## Summary

This RFD replaces lazy conversation loading with an eager, command-driven model.
Commands declare what data they need (index only, metadata, or full events) and
which conversations they need it for.
The startup pipeline loads and validates that data before the command runs.
After startup, access to loaded data is infallible — no `Result` or `Option` at
call sites for built-in commands.

## Motivation

### Lazy loading spreads fallibility everywhere

The workspace currently lazy-loads conversation metadata and events through
`OnceLock` cells.
Every access returns a `Result` because the underlying filesystem read might
fail at any point during command execution:

```rust
// Current: every caller must handle errors
let metadata = workspace.metadata(&handle)?;
let events = workspace.events(&handle)?;
```

This `Result` proliferation exists because loading is deferred to the moment of
first access — a filesystem read that can fail for any number of reasons (I/O
error, corrupt file, file deleted between index scan and access).
Every command that touches conversation data must handle these errors, even
though the expected failure rate after a successful sanitize pass is near zero.

### Lazy loading exists for one use case

The lazy-loading design exists primarily because `jp conversation ls` loads
metadata for all conversations to display a list.
Eagerly parsing every `events.json` at startup would be wasteful since `ls` only
needs lightweight stats (event count, last timestamp) — not the full
deserialized event stream.

But this is a data granularity problem, not a loading timing problem.
The solution is to split *what* is loaded, not *when*.

### Built-in commands know what they need at declaration time

The `ConversationLoadRequest` mechanism already captures which conversations a
command targets.
For built-in commands, the set of conversations whose events need loading is
determined entirely by the command and its arguments — it is known before
command execution begins.
Even bulk commands like `conversation grep` (all conversations) and
`conversation rm --from/--until` (a filtered subset) can express their scope
through `filter_needs` — the data requirements are known statically, only the
exact IDs are resolved at startup.

Two mechanisms serve bulk commands:

- **`ConversationTarget::All`** — the pipeline resolves `All` to handles for
  every indexed conversation during target resolution, using only the index (no
  metadata needed).
  Phase 2 then loads `target_needs` for each handle strictly.
  Use this when every conversation must load successfully.

- **`filter_needs`** — the filter phase loads data for all indexed
  conversations with skip-and-warn semantics (best-effort).
  Use this for commands like `conversation grep` that should silently skip
  corrupt conversations rather than abort.

This means the startup pipeline has all the information it needs to load
everything eagerly, fail cleanly if anything is wrong, and hand the command an
infallible view of its data.

External plugin commands are an exception: they declare
`ConversationLoadRequest::none()` at startup, but their RPC protocol allows them
to request conversation listings and event streams at runtime.
Plugin commands are explicitly out of scope for the infallible access guarantee
— they continue to use the fallible escape hatch API (see [Fallible escape
hatch for late-discovered
data](#fallible-escape-hatch-for-late-discovered-data)).
Infallible access for plugin commands could be achieved through a plugin-side
data declaration mechanism, but that is future work.

## Design

### Command data requirements

Commands declare what data they need through an extended
`ConversationLoadRequest`.
Two new dimensions are added:

1. **Filter needs** — what data the pipeline must load for all indexed
   conversations before target resolution (so the command can filter, sort, or
   pick).
2. **Target needs** — what data the pipeline must load for the resolved target
   conversations before the command runs.

Both use the same `DataNeeds` flags type:

```rust
/// What data to load for a set of conversations.
///
/// Flags are independent and composable. The conversation ID is always
/// available from the index scan and does not need a flag.
struct DataNeeds {
    /// Load `metadata.json` + lightweight event stats.
    pub metadata: bool,

    /// Load the full event stream (`events.json` + `base_config.json`).
    pub events: bool,
}

impl DataNeeds {
    pub const NONE: Self = Self { metadata: false, events: false };
    pub const METADATA: Self = Self { metadata: true, events: false };
    pub const FULL: Self = Self { metadata: true, events: true };
}
```

Per-conversation config loading is handled separately from `DataNeeds`.
The existing `config_conversation` field on `ConversationLoadRequest` already
identifies *which* handle to use for config loading.
When set, the pipeline loads `base_config.json` and parses `"type":
"config_delta"` events from `events.json` for that handle, builds the merged
`AppConfig`, and feeds it into the config pipeline — all before the command
runs.

This is deliberately not a `DataNeeds` flag because it is not a data access
declaration.
No command calls `handle.config()` during `run()`.
Config loading is a startup pipeline concern: it consumes event data to produce
an `AppConfig`, then discards the intermediate result.
If the same handle also declares `target_needs = FULL` (as `jp query` does), the
full stream load subsumes the config-only parse; the pipeline does not load
twice.

Initially, the config-loading step may load the full `events.json` and filter
for config deltas post-parse.
A future optimization could parse only `"type": "config_delta"` objects,
skipping expensive deserialization and base64 decoding of chat messages and tool
calls.

The request carries filter needs and target needs separately:

```rust
struct ConversationLoadRequest {
    /// Data needed for ALL indexed conversations before target resolution.
    /// Used by commands that filter, sort, or display conversation lists.
    pub filter_needs: DataNeeds,

    /// Data needed for the resolved target conversations.
    pub target_needs: DataNeeds,

    // existing fields...
}
```

The pipeline loads data in two passes: first `filter_needs` for all indexed
conversations, then `target_needs` for resolved targets.
If a target was already loaded during filtering, it is not loaded again.

`filter_needs` is a floor, not a ceiling.
Each `ConversationTarget` variant declares its own resolution needs:

```rust
impl ConversationTarget {
    fn resolution_needs(&self) -> DataNeeds {
        match self {
            Self::Id(_) | Self::All => DataNeeds::NONE,
            Self::Latest | Self::LatestPinned => DataNeeds::METADATA,
            Self::Newest | Self::AllSession => DataNeeds::NONE,
            Self::Picker(_) | Self::AllPinned => DataNeeds::METADATA,
            Self::SessionPrevious | Self::Help => DataNeeds::NONE,
        }
    }
}
```

`Newest` resolves via `id.timestamp()`, which is encoded in the `ConversationId`
and available from the index scan alone.
`AllSession` resolves from the session mapping and the index.
Neither requires metadata.

`All` and `Range` are new variants introduced by this RFD; the existing variants
reflect the current `ConversationTarget` enum in
`crates/jp_cli/src/cmd/target.rs`.

The pipeline merges three sources to compute effective filter needs:

1. The command's declared `filter_needs`.
2. The `resolution_needs()` of all `ConversationTarget` values in the request.
3. The `resolution_needs()` of the configured `DefaultConversationId`, which
   provides the fallback target when no explicit target is given and no session
   is active.

The third source is necessary because `resolve_from_session_or_picker` calls
`resolve_default_id` before falling through to the picker.
The `DefaultConversationId` config value maps to `ConversationTarget` variants
(`LastActivated` → `Latest`, `LastCreated` → `Newest`, `Previous` →
`SessionPrevious`), each with their own resolution needs.
The pipeline must account for these even when the request carries no explicit
targets.

The `default_id` value is already extracted from config before target resolution
(in `run_inner`), so it is available to the pipeline at this stage.

A command with `filter_needs: NONE` that uses a `Picker` target still gets
metadata loaded for all conversations.

Because resolution needs are declared on the target variant itself, adding a new
`ConversationTarget` variant forces the author to add a match arm.
The compiler ensures this — no separate inference function to keep in sync.

Target needs apply uniformly to all resolved targets.
Per-target granularity is supported at the type level (the underlying storage
can map target → needs) but no current command requires it.
A builder API makes the common case trivial:

```rust
impl ConversationLoadRequest {
    /// Set the same data needs for all resolved targets.
    fn with_target_needs(mut self, needs: DataNeeds) -> Self;
}
```

Example declarations:

| Command                 | `filter_needs` | `target_needs` | Notes                                  |
| ----------------------- | -------------- | -------------- | -------------------------------------- |
| `jp conversation ls`    | `METADATA`     | `NONE`         |                                        |
| `jp conversation path`  | `NONE`         | `NONE`         |                                        |
| `jp conversation show`  | `NONE`         | `METADATA`     | Currently `FULL`; see Phase 3          |
| `jp conversation print` | `NONE`         | `FULL`         |                                        |
| `jp conversation grep`  | `FULL`         | `NONE`         | No-target mode (best-effort)           |
| `jp conversation grep`  | `NONE`         | `FULL`         | With explicit targets (strict)         |
| `jp query`              | `NONE`         | `FULL`         | Acquires lock                          |
| `jp conversation fork`  | `NONE`         | `FULL`         | Reads source metadata + events         |
| `jp conversation edit`  | `NONE`         | `FULL`         | Acquires lock                          |
| `jp conversation rm`    | `METADATA`     | `FULL`         | Range: filter + resolve; acquires lock |
| `jp config set -c`      | `NONE`         | `FULL`         | Acquires lock                          |

Commands that acquire a `ConversationLock` must declare `target_needs = FULL`
because the lock type holds both metadata and events (see [Lock acquisition and
typed handles](#lock-acquisition-and-typed-handles)).

For `ls`, `filter_needs` is `METADATA` because it filters by `--local`,
`--limit` (sorted by activity), and pinned status — all of which require
metadata.
After filtering and sorting, `ls` displays data from metadata (title, event
count, last activity).
It never needs the full event stream, so `target_needs` is `NONE` — filtering
already loaded everything it needs.

For `grep`, the data requirements depend on whether explicit targets are given.
When no targets are provided, all indexed conversations are the effective target
set.
The command declares `filter_needs: FULL` so the filter phase loads every
conversation's events best-effort (skip-and-warn on failure, matching current
behavior where `workspace.events()` failures are logged and the conversation is
skipped).
`run()` then iterates the already-loaded data.
When explicit targets are given, `filter_needs` drops to `NONE` and
`target_needs: FULL` handles the load strictly.
The `conversation_load_request()` method will need to branch on whether
`self.target` has explicit IDs — it currently returns `explicit_or_none`, which
does not yet make this distinction.
The table shows both modes as separate rows.

For `rm --from/--until`, the range filter currently loads all conversations
inside `run()` and does not actually filter by the `from`/`until` values — this
is a pre-existing gap in the implementation.
With the new model, range mode should declare `filter_needs: METADATA` (so
metadata is available for date-based filtering) and a target set derived from
the range.
The pipeline must narrow the target set *before* Phase 2, not after — otherwise
`target_needs: FULL` would load events for every conversation, defeating the
purpose of range filtering.
The range resolution should happen during step 5 (target resolution), using the
metadata loaded in step 4 to select only the conversations within the
`from`/`until` bounds.
Phase 2 then loads events only for the narrowed set.

The mechanism: `conversation_load_request()` returns a
`ConversationTarget::Range(from, until)` variant.
During step 5, the pipeline resolves `Range` by scanning all indexed
conversations (using their IDs, which encode creation timestamps) and selecting
those within the bounds.
This resolution needs only the index — no metadata — because conversation IDs
already encode the creation timestamp.
The `filter_needs: METADATA` is still needed so the confirmation display in
`run()` can show conversation titles and event counts.

### Two-phase startup pipeline

The startup pipeline becomes a two-phase negotiation between the command and the
loading infrastructure:

**Phase 1 — Targeting:**

1. Sanitize the data store (existing behavior, unchanged).
2. Scan the conversation index (directory listing → IDs).
3. Compute effective filter needs: merge the command's `filter_needs` with the
   `resolution_needs()` of all `ConversationTarget` values in the request and
   the configured `DefaultConversationId`.
4. Load effective filter needs for all indexed conversations.
   If the effective needs are `NONE`, this step is skipped.
   Failures during filter loading are logged and the conversation is skipped
   (best-effort), preserving the current behavior where `jp conversation ls` and
   picker-based flows silently omit conversations with corrupt or missing files.
5. Resolve the command's targets to concrete conversation IDs.
   The loaded metadata is available for the picker, sort, and filter logic.

**Phase 2 — Data loading:**

6. For each resolved target, load `target_needs` minus whatever filtering
   already provided.
   If filtering already loaded metadata and `target_needs` only requires
   metadata, nothing more is needed.
   If `target_needs` includes events, load the full event stream.
7. If any target load fails, surface the error and abort before the command
   runs.
   Unlike filter loading, target loading is strict: the user explicitly asked
   for this conversation, so a failure is an error, not a warning.
8. Command runs with infallible access to all declared data.

All filesystem I/O happens in phases 1–6.
After phase 7, the command operates on in-memory data with no `Result` on
access.

The distinction between best-effort (filter) and strict (target) loading
preserves backwards compatibility for scan-all operations while providing the
fail-fast semantics that matter for targeted commands.
A TOCTTOU race (file deleted between sanitize and load) in the filter phase
produces a warning and an omitted conversation; the same race in the target
phase produces a clear error before the command runs.

### Infallible access after startup

After the startup pipeline completes, access to loaded data is infallible.
The current fallible API:

```rust
// Current: every access can fail
fn metadata(&self, h: &ConversationHandle) -> Result<RwLockReadGuard<Conversation>>;
fn events(&self, h: &ConversationHandle) -> Result<RwLockReadGuard<ConversationStream>>;
```

becomes infallible for data that was declared and loaded.
The `OnceLock` machinery is removed.

#### Typed handles

The current `ConversationHandle` is an untyped token — it proves a conversation
exists in the index but says nothing about what data was loaded.
With eager loading, a handle produced by the pipeline should encode what data is
available, so that calling `events()` on a metadata-only handle is a compile
error rather than a runtime panic.

One promising approach is const generics:

```rust
struct ConversationHandle<const HAS_METADATA: bool, const HAS_EVENTS: bool> {
    id: ConversationId,
}

// metadata() only exists when HAS_METADATA is true
impl<const E: bool> ConversationHandle<true, E> {
    fn metadata(&self) -> RwLockReadGuard<Conversation> { ... }
}

// events() only exists when HAS_EVENTS is true
impl<const M: bool> ConversationHandle<M, true> {
    fn events(&self) -> RwLockReadGuard<ConversationStream> { ... }
}
```

The pipeline promotes handles as data is loaded (`with_metadata()`,
`with_events()`), and each command's `run` method declares the handle type it
expects.
The compiler enforces that commands only access data they declared.

#### Promotion boundary

The bridge between runtime resolution and compile-time types lives in the
dispatch layer (`crates/jp_cli/src/cmd.rs`).
The pipeline returns opaque, untyped handles.
The `Commands` match block promotes them to typed handles based on which command
is being dispatched — since each match arm knows exactly which command it's
calling, it can safely promote:

```rust
match self {
    Commands::Query(args) => {
        let typed = opaque_handles.into_iter()
            .map(|h| h.assume_full())
            .collect();
        args.run(ctx, typed).await
    }
    Commands::Conversation(Ls(args)) => {
        let typed = opaque_handles.into_iter()
            .map(|h| h.assume_metadata())
            .collect();
        args.run(ctx, typed).await
    }
    Commands::Conversation(Path(args)) => {
        args.run(ctx, opaque_handles).await  // no promotion needed
    }
}
```

The `assume_*` methods panic if the data wasn't loaded, but this panic is
isolated to a single auditable location (the dispatch layer) directly adjacent
to the static declaration of the command's requirements.
Deep inside the command's implementation, the developer works with typed handles
and the compiler enforces what's accessible.

The exact type mechanism (const generics, separate handle structs like
`MetadataHandle` and `FullHandle`, trait-based approaches) needs validation
against the actual codebase — picker handle creation and background task access
patterns may impose constraints that favor one approach over another.
This RFD establishes the goal (type-level encoding of data availability with
infallible access) and the architectural boundary (promotion at dispatch)
without committing to a specific type mechanism.

#### Lock acquisition and typed handles

The current `ConversationLock` (defined in RFD 069) holds both
`Arc<RwLock<Conversation>>` and `Arc<RwLock<ConversationStream>>`.
Every mutating command — `query`, `config set`, `conversation edit`,
`conversation rm` — acquires a lock and accesses both metadata and events
through it.
`conversation fork` reads the source conversation's metadata and events directly
(without locking), then locks the newly created fork.
No current command acquires a lock for metadata-only mutation.

With typed handles, `Workspace::lock_conversation` accepts only a fully-loaded
handle:

```rust
impl Workspace {
    // Only a handle with both metadata and events loaded can be locked.
    pub fn lock_conversation(
        &self,
        handle: ConversationHandle<true, true>,
        session: Option<&Session>,
    ) -> Result<LockResult> { ... }
}
```

This enforces at compile time that any command acquiring a lock has declared
`target_needs = FULL`.
A command with a `ConversationHandle<true, false>` (metadata only) cannot call
`lock_conversation` — the compiler rejects it.

The `ConversationLock` internals are unchanged: it continues to hold both
metadata and events, and its `as_mut()` / `into_mut()` API remains as defined in
RFD 069.
The typed handle system provides the enforcement; the lock system provides the
mutation semantics.

This conflates three concerns: exclusive access (the flock), data loading
(parsing files into memory), and mutation capability (write access to the parsed
data).
Today's `lock_conversation` does all three atomically — it acquires the flock,
force-loads both metadata and events, and returns a type that provides read and
write access to both.
With eager loading the data is already in memory, but the lock signature still
forces `FULL` because the `ConversationLock` struct holds both
`Arc<RwLock<Conversation>>` and `Arc<RwLock<ConversationStream>>`.

This means commands that only need exclusion — like `conversation rm`, which
locks to prevent concurrent access during deletion and displays a confirmation
using data already available in metadata (`events_count`, `last_event_at`) —
are forced to declare `target_needs = FULL` and pay for full event
deserialization they don't use.

The practical cost is small: these are low-frequency interactive operations
where a few hundred milliseconds of extra parsing is imperceptible.
As an immediate mitigation, commands like `rm` and `show` should be refactored
to read `events_count` and `last_event_at` from metadata rather than loading and
iterating the full event stream.

A more principled fix would decouple lock acquisition from data requirements.
One direction: `lock_conversation` accepts any handle type and returns a
`ConversationLock` that is generic over data availability.
The lock proves exclusion; the handle's type parameters prove data availability.
Creating a `ConversationMut` (which needs both metadata and events for its
persistence-on-drop behavior) would require a fully-loaded lock, but read-only
locked access and deletion would not.
This is a larger change to the RFD 069 type hierarchy and is deferred to future
work.

### Fallible escape hatch for late-discovered data

The infallible API covers the primary command path, but the workspace also
exposes fallible methods for loading data that was not declared at startup:

```rust
// Fallible: load data that wasn't part of the startup declaration
fn try_load_metadata(&self, id: &ConversationId) -> Result<RwLockReadGuard<Conversation>>;
fn try_load_events(&self, id: &ConversationId) -> Result<RwLockReadGuard<ConversationStream>>;
```

These read from the backing store on demand, cache the result in the workspace
state, and return a `Result`.
If the data was already loaded eagerly, they return it without hitting disk.

Two categories of code use this API:

1. **Background tasks** — title generation runs after the command starts and
   needs to lock and mutate a conversation that was part of the original
   request.
   `TitleGeneratorTask::sync` calls `acquire_conversation`, `lock_conversation`,
   and `update_metadata`.
   Because this runs after the main command has released its lock, the escape
   hatch needs a `try_lock_conversation` path that loads data and acquires the
   flock in one fallible step, returning the existing `ConversationLock` type.

2. **External plugin commands** — plugin RPC messages (`ListConversations`,
   `ReadEvents`) request conversation data at runtime.
   The plugin host (`crates/jp_cli/src/cmd/plugin/dispatch.rs`) handles these
   via `workspace.conversations()` and `workspace.events()`.
   With eager loading, the plugin listing path uses the best-effort metadata
   iterator, which should be exposed under a distinct name (e.g.,
   `try_conversations()`) to avoid confusion with the infallible startup-loaded
   `conversations()` method that built-in commands use.
   Per-conversation event loading routes through a per-ID fallible load
   (`try_load_events`).
   The plugin protocol already returns structured error responses for load
   failures.

The full fallible API surface is:

```rust
// Per-ID late loading (background tasks, plugin ReadEvents)
fn try_load_metadata(&self, id: &ConversationId) -> Result<RwLockReadGuard<Conversation>>;
fn try_load_events(&self, id: &ConversationId) -> Result<RwLockReadGuard<ConversationStream>>;

// Load + lock in one step (background tasks that need mutation)
fn try_lock_conversation(
    &self,
    id: &ConversationId,
    session: Option<&Session>,
) -> Result<LockResult>;

// Scan-all metadata iterator (plugin ListConversations, best-effort).
// Distinct from the infallible `conversations()` used by built-in commands.
fn try_conversations(&self) -> impl Iterator<Item = (&ConversationId, ...)>;
```

The expectation is that all built-in commands use the infallible API.
The fallible methods should be clearly documented as serving background tasks
and plugin host traffic — not as a general-purpose bypass of the
`ConversationLoadRequest` mechanism.

### Relationship to conversation repair

This RFD does not change the sanitize phase.
The current behavior (trash conversations with corrupt files) continues to work
with eager loading.

A more granular repair strategy — one that can rebuild missing metadata from
defaults, reconstruct config from the workspace pipeline, or offer the user an
editor to fix a JSON syntax error — would complement eager loading by reducing
the number of conversations lost to corruption before the loading pipeline runs.
That work is orthogonal and can be pursued independently.

### Removing the `OnceLock` cache

The current workspace state uses `OnceLock` for lazy initialization:

```rust
// Current
struct State {
    conversations: BTreeMap<ConversationId, OnceLock<Arc<RwLock<Conversation>>>>,
    events: BTreeMap<ConversationId, OnceLock<Arc<RwLock<ConversationStream>>>>,
}
```

This is replaced with direct storage for loaded data:

```rust
// Proposed
struct State {
    /// All known conversation IDs (from the index scan).
    index: BTreeSet<ConversationId>,

    /// Loaded metadata. Populated for conversations where the command
    /// or target resolution required metadata.
    metadata: BTreeMap<ConversationId, Arc<RwLock<Conversation>>>,

    /// Loaded event streams. Populated only for conversations where
    /// `target_needs` includes events.
    events: BTreeMap<ConversationId, Arc<RwLock<ConversationStream>>>,
}
```

`metadata()` and `events()` look up directly in these maps.
If the key is missing, the data was not loaded — which means either the command
didn't declare it needed that data (a bug), or loading failed and was caught
during startup (already handled).

## Drawbacks

**Command declarations must be accurate.** If the typed handle approach works
out, mismatches are caught at compile time.
If a simpler runtime approach is used instead, a command that accesses data it
didn't declare would panic.
The mitigation is that the set of commands is small, the data declarations are
co-located with the command definitions, and tests exercise the actual access
patterns.

## Alternatives

### Eager-load everything at startup

Parse all metadata and all event streams for all conversations at startup.
Access is infallible everywhere.

Rejected because event streams can be large (megabytes for long conversations)
and most commands only interact with 1-2 conversations.
Loading all events eagerly wastes memory and adds unnecessary startup latency.

### Bytes-in-memory with deferred parsing

Read raw file bytes eagerly at startup, validate structurally, but defer
deserialization to access time.

Rejected because structural validation (valid JSON) does not guarantee
successful deserialization into typed structs.
The parse can still fail, so the access still needs a `Result`, defeating the
purpose.

## Non-Goals

- **Cross-invocation caching.** This RFD does not introduce persistent caching
  of parsed conversation data across CLI invocations.
  Each invocation reads from disk.

- **Streaming or incremental event loading.** Loading a subset of events from a
  conversation (e.g., last N events) is a separate optimization opportunity.

- **Changes to the storage file format.** The `metadata.json`, `events.json`,
  and `base_config.json` files are unchanged.

- **`LoadBackend` trait changes.** The backend trait surface is unchanged.
  The eager-vs-lazy decision lives in the workspace and CLI layers, not the
  storage layer.

- **Conversation repair improvements.** Improving the sanitize phase to repair
  rather than trash recoverable conversations is valuable but orthogonal.

## Risks and Open Questions

### Target resolution depends on metadata

Some `ConversationTarget` variants (`Latest`, `LatestPinned`, `Pinned`,
`Picker`) require metadata to resolve.
Today they call `workspace.conversations()` which triggers lazy metadata loading
for all conversations.

With the new model, each `ConversationTarget` variant declares its own
`resolution_needs()` (see [Command data
requirements](#command-data-requirements)).
The pipeline merges these with the command's `filter_needs` automatically.
Adding a new variant that requires metadata means adding a match arm to
`resolution_needs()` — the compiler enforces exhaustive matching, so this
cannot be forgotten.

### Performance of eager metadata loading

The lightweight events scan (`load_count_and_timestamp_events`) reads the full
`events.json` file to count entries and find the last timestamp.
For conversations with thousands of events, this file can be hundreds of
kilobytes.
When `filter_needs` includes metadata, this scan runs for every conversation at
startup.

This is the same cost as today (the scan already runs during lazy metadata
loading via `ensure_all_metadata_loaded`), just moved earlier.
The current implementation uses rayon for parallel loading, and this continues
to apply.

For context: RFD 053 explicitly rejected O(N) metadata loading on the main
thread for `jp query` startup.
That concern does not apply here — `jp query` declares `filter_needs: NONE`, so
it loads metadata only for its single target conversation (O(1)).
The O(N) metadata scan only runs for commands like `jp conversation ls` and
picker-based resolution, which already pay this cost today.

Based on the current storage layout, workspaces with fewer than ~500
conversations should see no perceptible difference in startup latency.
For larger workspaces, the metadata scan could become a bottleneck.
The mitigation is to persist `events_count` and `last_event_at` in
`metadata.json`, avoiding the `events.json` scan entirely during metadata
loading.
A benchmark should be added in Phase 2 to measure startup latency across
workspace sizes (10, 100, 500, 1000 conversations) and establish a threshold for
when the `metadata.json` optimization becomes necessary.

### Filter needs inference for implicit fallback targets

When a command receives explicit targets, the pipeline knows exactly which
`ConversationTarget` variants are present — clap parsing resolves `?` to
`Picker`, literal IDs to `Id`, etc. before `conversation_load_request()` is
called.
For these cases, filter needs inference is exact.

However, when no target is provided (e.g., bare `jp query`), the request is
`targets: Some(vec![])`, and `resolve_targets` tries three fallbacks in order:

1. Session mapping (the active conversation for this terminal session).
2. The configured `conversation.default_id` (which maps to `ConversationTarget`
   variants like `Latest`, `Newest`, or `SessionPrevious`).
3. The interactive picker.

The pipeline doesn't know at request construction time which fallback will
succeed — that depends on runtime session state and config.
For these fallback paths, the pipeline must assume the worst case: it merges the
resolution needs of the configured `default_id` with the picker's metadata
requirement.
This is a narrow over-load (only affects commands with no explicit target and no
active session) and the cost is small (metadata loading is fast).

## Implementation Plan

### Phase 1: Add `DataNeeds` to `ConversationLoadRequest`

Add `filter_needs` and `target_needs` fields with defaults that preserve current
behavior (`filter_needs: NONE`, `target_needs: FULL`).
Annotate each command with its actual requirements.
Request construction for `grep` and `rm` will need to branch on CLI shape
(no-target vs explicit, range vs single), but the new declarations are inert
until Phase 2 consumes them.
No loading behavior changes yet.

**Depends on:** Nothing.
**Mergeable:** Yes.
No loading behavior change.

### Phase 2: Eager loading pipeline

Change the startup pipeline in `run_inner` to consume the new
`ConversationLoadRequest` fields:

- Load `filter_needs` for all indexed conversations before target resolution.
- After target resolution, load `target_needs` for resolved targets.
- Remove the `OnceLock` machinery from workspace state.
- Change `metadata()` and `events()` to return infallible types.
- Update `Workspace::conversations()` to no longer perform I/O — it yields only
  what Phase 1 loaded.
  Remove `ensure_all_metadata_loaded`.
- Add a startup latency benchmark measuring metadata + event loading across
  workspace sizes (10, 100, 500, 1000 conversations).
  Establish a threshold for when the `metadata.json` stat-caching optimization
  becomes necessary.

**Depends on:** Phase 1 (data requirements must be declared).
**Mergeable:** Yes, but this is the large change.

### Phase 3: Clean up callers

Remove error handling from command implementations that access metadata or
events.
Refactor `conversation show` and `conversation rm` to read `events_count` and
`last_event_at` from metadata rather than loading and iterating the full event
stream.
Update tests.

Note: the command table in this RFD reflects the post-Phase-3 state.
During Phase 1, `show` should be annotated as `target_needs = FULL` (its current
behavior).
The Phase 3 refactor changes it to `METADATA`.

**Depends on:** Phase 2.
**Mergeable:** Yes.

## References

- [RFD 052] — Workspace Data Store Sanitization.
  Defines the current sanitize and trash behavior.
- [RFD 053] — Auto-Refresh Conversation Titles.
  Documents the startup-cost tension around O(N) metadata loading.
- [RFD 054] — Split Conversation Config and Events.
  Established the three-file conversation storage layout (`metadata.json`,
  `events.json`, `base_config.json`).
- [RFD 069] — Guard-Scoped Persistence for Conversations.
  Defines the `ConversationLock` / `ConversationMut` type hierarchy and
  persistence model.
- [RFD 073] — Layered Storage Backend for Workspaces.
  Defines the `LoadBackend` trait through which conversation data is loaded.

[RFD 052]: 052-workspace-data-store-sanitization.md
[RFD 053]: 053-auto-refresh-conversation-titles.md
[RFD 054]: 054-split-conversation-config-and-events.md
[RFD 069]: 069-guard-scoped-persistence-for-conversations.md
[RFD 073]: 073-layered-storage-backend-for-workspaces.md
