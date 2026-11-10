# RFD 074: Eager Loading with Command-Declared Data Requirements

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-15
- **Summary**: Loads and validates command-declared conversation data eagerly at
  startup so command execution accesses loaded state infallibly.

## Summary

This RFD replaces lazy conversation loading with an eager, command-driven model.
Each command declares which conversations and what data granularity (index,
metadata, or events) it requires.
The startup pipeline loads and validates that data before the command runs, so
command execution accesses loaded state infallibly.

## Motivation

### Lazy loading makes every access fallible

`Workspace` lazy-loads conversation metadata and events through `OnceLock`
cells.
Because the filesystem read is deferred to first access, every call returns a
`Result`:

```rust
// Current: every caller must handle errors
let metadata = workspace.metadata(&handle)?;
let events = workspace.events(&handle)?;
```

First access can fail on an I/O error, a corrupt file, or a file deleted between
the index scan and the read.
Every command that touches conversation data must propagate these errors, even
though failures after the startup sanitize pass are rare.

### Lazy loading solves a granularity problem with timing

Lazy loading was introduced because `jp conversation ls` lists every
conversation in the workspace.
Eagerly parsing every `events.json` at startup would be wasteful when `ls` only
needs lightweight stats (event count and last timestamp) rather than the full
deserialized event stream.

Deferring reads to first access avoids parsing unneeded event streams, but the
underlying problem is data granularity, not load timing.
Separating *what* is loaded (index, metadata, or events) lets the startup
pipeline load only what `ls` needs.

### Built-in commands know what they need at declaration time

`ConversationLoadRequest` already captures which conversations a command
targets.
For built-in commands, the command and its arguments determine both the target
set and the required data before execution begins.
Even bulk commands like `conversation grep` (all conversations) and
`conversation rm --created-since/--created-before` (a filtered subset) have
static data requirements; only the matching conversation IDs are resolved at
startup.

Two mechanisms serve bulk commands:

- **`ConversationTarget::All`**: resolves to handles for every indexed
  conversation during target resolution using only the index (no metadata
  needed), then loads `target_needs` strictly for each handle in Phase 2.
  Use this when every conversation must load successfully.

- **`filter_needs`**: loads data for all indexed conversations during the filter
  phase with best-effort skip-and-warn semantics.
  Use this for commands like `conversation grep` that skip corrupt conversations
  rather than abort.

With these declarations, the startup pipeline can load all required data
eagerly, fail before execution if a target cannot be loaded, and hand the
command an infallible view of its data.

External plugin commands are an exception: they declare
`ConversationLoadRequest::none()` at startup and request conversation listings
and event streams at runtime over RPC.
Plugin commands are out of scope for infallible access and continue to use the
fallible escape hatch API (see [Fallible escape hatch for late-discovered
data](#fallible-escape-hatch-for-late-discovered-data)).
A plugin-side data declaration mechanism is left to future work.

## Design

### Command data requirements

Commands declare what data they need through two `DataNeeds` fields on
`ConversationLoadRequest`:

1. **Filter needs**: data loaded for all indexed conversations before target
   resolution so the pipeline or command can filter, sort, or pick.
2. **Target needs**: data loaded for the resolved target conversations before
   the command runs.

Both fields use the `DataNeeds` flags type:

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

Per-conversation config loading stays separate from `DataNeeds`.
The existing `config_conversation` field on `ConversationLoadRequest` identifies
which handle supplies conversation config.
When set, the startup pipeline loads `base_config.json`, parses `"type":
"config_delta"` events from `events.json` for that handle, builds the merged
`AppConfig`, and feeds it into the config pipeline before the command runs.

Config loading is not a `DataNeeds` flag because commands do not call
`handle.config()` during `run()`; the pipeline consumes the event data to
produce an `AppConfig` and discards the intermediate result.
If the same handle also declares `target_needs = FULL` (as `jp query` does), the
full stream load subsumes the config parse so the file is not read twice.

Initially, the config-loading step can load the full `events.json` and filter
for config deltas after parsing.
A later optimization can parse only `"type": "config_delta"` objects, skipping
deserialization and base64 decoding of chat messages and tool calls.

The request carries filter needs and target needs as separate fields:

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
Data already loaded during filtering is not reloaded in the second pass.

The command's `filter_needs` sets a minimum requirement.
Each `ConversationTarget` variant also declares the data it needs to resolve:

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

`Newest` resolves from `id.timestamp()`, which is encoded in `ConversationId`
and available from the index scan alone.
`AllSession` resolves from the session mapping and the index.
Neither requires metadata.
`All` and `Range` are new variants introduced by this RFD; the remaining
variants match the existing `ConversationTarget` enum in
`crates/jp_cli/src/cmd/target.rs`.
Because resolution needs live on the enum itself, adding a `ConversationTarget`
variant requires a match arm in `resolution_needs()`, keeping target resolution
and data loading in sync at compile time.

The pipeline merges three sources to compute effective filter needs:

1. The command's declared `filter_needs`.
2. The `resolution_needs()` of every `ConversationTarget` in the request.
3. The `resolution_needs()` of the configured `DefaultConversationId`, which
   supplies the fallback target when no explicit target is given and no session
   is active.

Accordingly, a command with `filter_needs: NONE` that uses a `Picker` target
still loads metadata for all conversations.
The third source is required because `resolve_from_session_or_picker` calls
`resolve_default_id` before falling back to the picker.
`DefaultConversationId` maps to `ConversationTarget` variants (`LastActivated`
→ `Latest`, `LastCreated` → `Newest`, `Previous` → `SessionPrevious`), each
with its own resolution needs, and `default_id` is already extracted from config
in `run_inner` before target resolution begins.

Target needs apply uniformly to all resolved targets.
The underlying storage can map individual targets to distinct `DataNeeds`, but
no current command requires per-target granularity.
A builder method covers the common case:

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
because the lock holds both metadata and events (see [Lock acquisition and typed
handles](#lock-acquisition-and-typed-handles)).

For `ls`, `filter_needs` is `METADATA` because filtering by `--local`, `--limit`
(sorted by activity), and pinned status requires metadata, as does rendering the
list (title, event count, and last activity).
Because `ls` never reads the full event stream, `target_needs` is `NONE`.

For `grep`, the declaration depends on whether explicit targets are given (shown
as separate rows in the table):

- **Without targets**: all indexed conversations are searched.
  The command declares `filter_needs: FULL` and `target_needs: NONE` so the
  filter phase loads every conversation's events best-effort (logging failures
  and skipping corrupt conversations, matching current `workspace.events()`
  behavior), and `run()` iterates the loaded streams.
- **With explicit targets**: `filter_needs` is `NONE` and `target_needs: FULL`
  loads the requested conversations strictly.

`conversation_load_request()` currently returns `explicit_or_none` and must
branch on whether `self.target` contains explicit IDs.

For `rm --created-since/--created-before`, the range filter currently loads all
conversations inside `run()` without filtering by the `since`/`before` bounds (a
pre-existing gap).
In the new model, `conversation_load_request()` returns a
`ConversationTarget::Range(from, until)` variant and declares `filter_needs:
METADATA` and `target_needs: FULL`.
During target resolution (step 5), the pipeline resolves `Range` by scanning all
indexed conversation IDs, which encode creation timestamps, and selecting those
within the `from`/`until` bounds.
Resolving the range needs only the index, not metadata, and narrows the target
set before Phase 2 so `target_needs: FULL` loads events only for matching
conversations.
`filter_needs: METADATA` is still declared so the confirmation prompt in `run()`
can display conversation titles and event counts.

### Two-phase startup pipeline

The startup pipeline runs in two phases:

**Phase 1: Targeting**

1. Sanitize the data store (existing behavior, unchanged).
2. Scan the conversation index (directory listing → IDs).
3. Compute effective filter needs by merging the command's `filter_needs` with
   the `resolution_needs()` of all `ConversationTarget` values in the request
   and the configured `DefaultConversationId`.
4. Load effective filter needs for all indexed conversations, or skip this step
   when effective needs are `NONE`.
   Filter loading is best-effort: failures are logged and the conversation is
   skipped, preserving the current behavior where `jp conversation ls` and
   picker flows omit conversations with corrupt or missing files.
5. Resolve the command's targets to concrete conversation IDs using the loaded
   metadata for picker, sort, and filter logic.

**Phase 2: Data loading**

6. For each resolved target, load `target_needs` minus whatever Phase 1 already
   loaded.
   If Phase 1 loaded metadata and `target_needs` requires only metadata, no
   further I/O is needed; if `target_needs` includes events, load the full event
   stream.
7. If any target load fails, surface the error and abort before the command
   runs.
   Target loading is strict because the user requested that specific
   conversation.
8. Run the command with infallible access to all declared data.

All filesystem I/O happens in steps 1 through 6.
After step 7, the command operates on in-memory data with no `Result` on access.

Best-effort filter loading and strict target loading preserve backwards
compatibility for scan-all operations while failing fast on targeted commands.
A TOCTTOU race (a file deleted between sanitize and load) in the filter phase
logs a warning and omits the conversation; the same race in the target phase
aborts with an error before the command runs.

### Infallible access after startup

Once the startup pipeline finishes, accessing declared data is infallible and
the `OnceLock` cache is removed.
The current fallible accessors:

```rust
// Current: every access can fail
fn metadata(&self, h: &ConversationHandle) -> Result<RwLockReadGuard<Conversation>>;
fn events(&self, h: &ConversationHandle) -> Result<RwLockReadGuard<ConversationStream>>;
```

are replaced by infallible methods on typed handles.

#### Typed handles

The current `ConversationHandle` is an untyped token: it proves a conversation
exists in the index, but not what data was loaded for it.
With eager loading, handles encode which data is present so calling `events()`
on a metadata-only handle is a compile error rather than a runtime panic.

One option is const generics:

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

The dispatch layer (`crates/jp_cli/src/cmd.rs`) bridges runtime loading and
compile-time handle types.
The startup pipeline returns untyped handles, and each `Commands` match arm
promotes them to the handle type its command expects:

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

The `assume_*` methods panic if the required data was not loaded, isolating that
runtime check to the dispatch layer right next to the command's static data
declaration.
Inside `run()`, commands receive typed handles and the compiler enforces which
accessors are available.

The exact representation (const generics, separate `MetadataHandle` and
`FullHandle` structs, or traits) depends on constraints from picker handle
creation and background tasks.
This RFD fixes the goal (compile-time encoding of loaded data with infallible
access) and the promotion boundary (command dispatch) without mandating a
specific type encoding.

#### Lock acquisition and typed handles

`ConversationLock` ([RFD 069]) holds both `Arc<RwLock<Conversation>>` and
`Arc<RwLock<ConversationStream>>`, and its `as_mut()` / `into_mut()` mutation
API stays unchanged.
Every mutating command (`query`, `config set`, `conversation edit`,
`conversation rm`) acquires a lock to access metadata and events; `conversation
fork` reads the source conversation's metadata and events without locking and
then locks the new fork.
No current command locks a conversation for metadata-only mutation.

With typed handles, `Workspace::lock_conversation` accepts only a fully loaded
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

Requiring `ConversationHandle<true, true>` enforces at compile time that any
command acquiring a lock declares `target_needs = FULL`.
Calling `lock_conversation` with a metadata-only `ConversationHandle<true,
false>` is a compile error.

Because `ConversationLock` holds both streams, this signature still couples
three concerns that current `lock_conversation` performs atomically: exclusive
access (the flock), data loading, and write access to both metadata and events.
With eager loading, the data is already in memory, but commands that only need
exclusion, such as `conversation rm` (which locks to prevent concurrent access
during deletion and prints confirmation details from metadata's `events_count`
and `last_event_at`), must still declare `target_needs = FULL` and deserialize
the event stream.

The practical cost is small for infrequent interactive commands.
As an immediate cleanup, `rm` and `show` will read `events_count` and
`last_event_at` from metadata instead of iterating the event stream.
Decoupling lock acquisition from data requirements, for example by making
`ConversationLock` generic over the handle's loaded data and requiring a fully
loaded lock only when constructing `ConversationMut` (which persists both files
on drop), is a larger change to the [RFD 069] type hierarchy and is left to
future work.

### Fallible escape hatch for late-discovered data

The infallible API covers built-in command execution, while fallible methods
remain on `Workspace` for callers that discover conversation IDs after startup:

```rust
// Fallible: load data that wasn't part of the startup declaration
fn try_load_metadata(&self, id: &ConversationId) -> Result<RwLockReadGuard<Conversation>>;
fn try_load_events(&self, id: &ConversationId) -> Result<RwLockReadGuard<ConversationStream>>;
```

These methods read from the backing store on demand, cache the loaded value in
workspace state, and return a `Result`.
If the data was already loaded at startup, they return the cached entry without
reading from disk.

Two callers use this API:

1. **Background tasks**: title generation runs after the command starts and
   locks and mutates a conversation from the original request.
   `TitleGeneratorTask::sync` calls `acquire_conversation`, `lock_conversation`,
   and `update_metadata`.
   Because it runs after the main command releases its lock, it needs a
   `try_lock_conversation` method that loads data and acquires the flock in one
   fallible step, returning `ConversationLock`.

2. **External plugin commands**: plugin RPC messages (`ListConversations`,
   `ReadEvents`) request conversation data at runtime.
   The plugin host (`crates/jp_cli/src/cmd/plugin/dispatch.rs`) currently serves
   these through `workspace.conversations()` and `workspace.events()`.
   With eager loading, `ListConversations` uses a best-effort metadata iterator
   under a distinct name (`try_conversations()`) to separate it from the
   infallible `conversations()` iterator used by built-in commands, while
   `ReadEvents` calls `try_load_events`.
   The plugin protocol already returns structured error responses on load
   failure.

The complete fallible API is:

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

All built-in commands use the infallible API.
The `try_*` methods exist only for background tasks and the plugin host, not as
a bypass of `ConversationLoadRequest`.

### Relationship to conversation repair

This RFD leaves the startup sanitize phase unchanged: conversations with corrupt
files are still moved to the trash before loading begins.
Richer repair (rebuilding missing metadata from defaults, reconstructing config
from the workspace pipeline, or opening an editor to fix JSON syntax errors)
would reduce how many conversations are trashed before the loader runs, and can
be pursued independently.

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

`metadata()` and `events()` read directly from these maps.
Because startup aborts when a target fails to load, a missing key at access time
means the command did not declare that data requirement (a bug).

## Drawbacks

**Command declarations must match actual access.** Typed handles catch
mismatches at compile time; with untyped handles, accessing undeclared data
panics at runtime.
Even without compile-time enforcement, the command set is small, declarations
live next to command definitions, and integration tests exercise the access
paths.

## Alternatives

### Eager-load everything at startup

Parse metadata and event streams for every conversation at startup so all access
is infallible without per-command declarations.

Rejected because event streams can reach several megabytes for long
conversations while most commands touch only one or two conversations.
Loading every event stream wastes memory and adds startup latency.

### Bytes-in-memory with deferred parsing

Read raw file bytes at startup, validate that they are well-formed JSON, and
defer deserialization until first access.

Rejected because valid JSON syntax does not guarantee deserialization into typed
structs.
Deserialization can still fail at access time, so accessors must still return
`Result`.

## Non-Goals

- **Cross-invocation caching.** Each CLI invocation reads from disk; parsed
  conversation data is not cached across invocations.

- **Streaming or incremental event loading.** Loading a subset of events (such
  as the last N events) is a separate optimization.

- **Changes to the storage file format.** `metadata.json`, `events.json`, and
  `base_config.json` are unchanged.

- **`LoadBackend` trait changes.** The storage trait surface is unchanged; eager
  loading is coordinated in the workspace and CLI layers.

- **Conversation repair improvements.** Repairing rather than trashing
  recoverable conversations during sanitization is orthogonal.

## Risks and Open Questions

### Target resolution depends on metadata

Some `ConversationTarget` variants (`Latest`, `LatestPinned`, `Pinned`,
`Picker`) require metadata to resolve.
They currently call `workspace.conversations()`, which lazily loads metadata for
all conversations.
Under eager loading, each variant declares its requirement in
`resolution_needs()` (see [Command data
requirements](#command-data-requirements)), and the pipeline merges those
requirements with the command's `filter_needs`.
Exhaustive matching on `ConversationTarget` ensures new variants declare their
resolution needs.

### Performance of eager metadata loading

Loading metadata runs `load_count_and_timestamp_events`, which scans each
conversation's `events.json` to count entries and read the last timestamp.
For conversations with thousands of events, `events.json` can be hundreds of
kilobytes.
When effective `filter_needs` includes metadata, this scan runs across all
indexed conversations at startup in parallel via rayon.

This is the same work that `ensure_all_metadata_loaded` performs lazily today,
moved to startup.
[RFD 053] rejected O(N) metadata loading on the main thread for `jp query`
startup, which still holds: `jp query` declares `filter_needs: NONE` and loads
metadata only for its resolved target (O(1)).
The O(N) scan runs only for commands like `jp conversation ls` and picker-based
target resolution, which already pay that cost.

Workspaces with fewer than ~500 conversations should see no perceptible startup
difference.
For larger workspaces, persisting `events_count` and `last_event_at` directly in
`metadata.json` would eliminate the `events.json` scan during metadata loading.
Phase 2 adds a benchmark across workspace sizes (10, 100, 500, and 1000
conversations) to establish when this optimization is needed.

### Filter needs inference for implicit fallback targets

When a command receives explicit targets, clap resolves `?` to `Picker`, literal
IDs to `Id`, and so on before `conversation_load_request()` runs, making
filter-needs inference exact.

When no target is provided (such as bare `jp query`), the request carries
`targets: Some(vec![])`, and `resolve_targets` tries three fallbacks in order:

1. The active conversation from the terminal session mapping.
2. The configured `conversation.default_id` (which maps to `ConversationTarget`
   variants such as `Latest`, `Newest`, or `SessionPrevious`).
3. The interactive picker.

Because the pipeline does not know at request construction time which fallback
will succeed, it assumes the worst case when no session is active, merging the
resolution needs of `default_id` with the picker's metadata requirement.
This over-loads metadata only when a command has neither an explicit target nor
an active session.

## Implementation Plan

### Phase 1: Add `DataNeeds` to `ConversationLoadRequest`

Add `filter_needs` and `target_needs` fields with defaults that preserve current
behavior (`filter_needs: NONE`, `target_needs: FULL`), and annotate each command
with its actual requirements.
Request construction for `grep` and `rm` branches on CLI shape (no-target versus
explicit targets, range versus single target), while the declarations remain
inert until Phase 2 consumes them.

**Depends on:** Nothing.
**Mergeable:** Yes.
No loading behavior change.

### Phase 2: Eager loading pipeline

Update the startup pipeline in `run_inner` to consume the
`ConversationLoadRequest` fields:

- Load `filter_needs` for all indexed conversations before target resolution.
- After target resolution, load `target_needs` for resolved targets.
- Remove the `OnceLock` cache from workspace state.
- Change `metadata()` and `events()` to return infallible types.
- Update `Workspace::conversations()` to yield only the metadata loaded during
  startup without performing I/O, and remove `ensure_all_metadata_loaded`.
- Add a startup latency benchmark measuring metadata and event loading across
  workspace sizes (10, 100, 500, and 1000 conversations) to establish when the
  `metadata.json` stat-caching optimization is needed.

**Depends on:** Phase 1 (data requirements must be declared).
**Mergeable:** Yes, but this is the large change.

### Phase 3: Clean up callers

Remove error handling from command implementations that access metadata or
events.
Refactor `conversation show` and `conversation rm` to read `events_count` and
`last_event_at` from metadata rather than loading and iterating the full event
stream, and update tests.

The command table in this RFD reflects the post-Phase-3 state: Phase 1 annotates
`show` with `target_needs = FULL` (its current behavior), and Phase 3 narrows it
to `METADATA`.

**Depends on:** Phase 2.
**Mergeable:** Yes.

## References

- [RFD 052]: Workspace Data Store Sanitization.
  Defines the current sanitize and trash behavior.
- [RFD 053]: Auto-Refresh Conversation Titles.
  Documents the startup-cost tension around O(N) metadata loading.
- [RFD 054]: Split Conversation Config and Events.
  Established the three-file conversation storage layout (`metadata.json`,
  `events.json`, `base_config.json`).
- [RFD 069]: Guard-Scoped Persistence for Conversations.
  Defines the `ConversationLock` / `ConversationMut` type hierarchy and
  persistence model.
- [RFD 073]: Layered Storage Backend for Workspaces.
  Defines the `LoadBackend` trait through which conversation data is loaded.

[RFD 052]: 052-workspace-data-store-sanitization.md
[RFD 053]: 053-auto-refresh-conversation-titles.md
[RFD 054]: 054-split-conversation-config-and-events.md
[RFD 069]: 069-guard-scoped-persistence-for-conversations.md
[RFD 073]: 073-layered-storage-backend-for-workspaces.md
