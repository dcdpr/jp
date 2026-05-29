# RFD 053: Auto-Refresh Conversation Titles

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-18
- **Requires**: [RFD 020], [RFD 069], [RFD 073]

## Summary

Conversation titles are generated once after the first turn and never
automatically updated.
This RFD adds periodic refresh: when
`conversation.title.generate.auto_refresh.turn_interval` is set to a positive
integer N (default 5), the least recently activated conversations that have
accumulated N new turns since their titles were last generated are re-titled as
background tasks on the next `jp query` run.

## Motivation

A title is generated on the first turn of a new conversation and then frozen.
This works well for short, focused conversations, but longer ones take
unexpected turns and end up with a title that describes only the opening
exchange.
The user is left with a list of conversations whose titles no longer reflect
what they contain.

The user can already run `conversation edit --title` to manually regenerate a
title, but this requires noticing the problem and taking action.
Periodic automatic refresh should be transparent.

The fix needs to be careful about cost.
Triggering a new LLM request on every `jp query` invocation for every stale
conversation in the workspace would spike API usage and potentially delay the
CLI on exit.
This design processes a bounded number of conversations per run in the
background — the same pattern already used for initial title generation.

## Design

### Configuration

A new `auto_refresh` sub-table is added to `conversation.title.generate`:

```toml
[conversation.title.generate]
auto = true
model = ...

[conversation.title.generate.auto_refresh]
turn_interval = 5   # refresh every N turns; 0 = disabled (default = 5)
batch_size = 1      # max conversations to refresh per run; or "all" (default = 1)
turn_context = 10   # max turns sent to LLM for re-titling; or false for unlimited (default = 10)
```

`turn_interval = 0` disables the feature entirely.

`batch_size` controls how many stale conversations are refreshed per `jp query`
invocation.
The default of `1` spreads the work across runs.
Setting it to `"all"` refreshes every stale conversation in a single run —
useful for catching up after enabling the feature on a workspace with many
long-running conversations, at the cost of more LLM requests.

`turn_context` limits how many recent turns are sent to the LLM when
re-generating a title.
For long conversations, earlier turns are often irrelevant to what the
conversation is currently about.
The default of `10` keeps costs predictable and focuses the title on recent
activity.
Setting it to `false` disables the limit and sends the full conversation.

These fields map to a new nested `AutoRefreshConfig` on `GenerateConfig`:

```rust
pub struct AutoRefreshConfig {
    pub turn_interval: usize,        // default 5
    pub batch_size: BatchSize,       // default Count(1)
    pub turn_context: Option<usize>, // default Some(10), None = unlimited
}

enum BatchSize {
    Count(usize),
    All,
}
```

Using `0` as a sentinel for "unlimited" would clash with `turn_interval`'s
`0`-as-disabled meaning — same struct, same sentinel, opposite direction.
`Option<usize>` keeps the natural reading: `Some(n)` means "send up to `n`
turns," `None` means "send all of them."

The TOML/JSON surface accepts the boolean `false` for the unlimited case.
`KvAssignment::try_some_u32` rejects boolean input, so a small
`try_some_u32_or_false` helper is added to `jp_config::assignment`, mirroring
the existing `try_some_bool_or_from_str` pattern: a non-negative integer maps to
`Some(n)`, the boolean `false` maps to `None`, and anything else (`true`, a
string, a negative integer) is rejected with a clear error.

When `conversation.title.generate.auto = false`, auto-refresh is also disabled
regardless of `turn_interval`.

### Persisted state: `title_generated_at_turn`

To determine staleness, the system needs to know how many turns existed when the
title was last generated.
A new optional field is added to `Conversation` in `metadata.json`:

```rust
/// Turn count when the title was last auto-generated.
///
/// - `None` — legacy conversation; treated as baseline 0 (auto-enrolls in
///   refresh once it has accumulated `turn_interval` turns).
/// - `Some(n)` — title was auto-generated when the conversation had `n` turns.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub title_generated_at_turn: Option<usize>,
```

This field is a pure watermark, not configuration.
It lives in `metadata.json` alongside `title` because they are tightly coupled
— one is the output, the other is the checkpoint that governs when it is
regenerated.

| Watermark | Title     | Meaning                              | Eligible?              |
| --------- | --------- | ------------------------------------ | ---------------------- |
| `None`    | `Some(_)` | Legacy conversation, has a title     | No (treated as manual) |
| `None`    | `None`    | Legacy conversation, no title        | Yes, baseline `0`      |
| `Some(n)` | (any)     | Auto-generated/evaluated at turn `n` | Yes, baseline `n`      |

#### Legacy conversations

Existing conversations created before this RFD have `title_generated_at_turn =
None`.
There is no provenance field that distinguishes a hand-titled conversation from
one whose title was generated automatically.
The migration policy is conservative:

- `title_generated_at_turn = None` and `title = Some(_)`: treated as manual and
  skipped by auto-refresh.
  The user (or a previous automatic generation) produced this title; without
  provenance the safe default is to leave it alone.
  A conversation enrolls into auto-refresh only after an explicit `conversation
  edit --title` (no argument) records a watermark.
- `title_generated_at_turn = None` and `title = None`: eligible.
  There is no title to overwrite, so auto-refresh proceeds with baseline `0`.

This trades a one-time "stale-but-not-refreshed" state for safety — hand-titled
conversations are preserved without guessing.

### Interaction with manual title surfaces

Several CLI paths set or clear `metadata.title` directly without going through
the LLM.
The user's intent in those cases is "this is the title I want" — the system
should not later overwrite it via auto-refresh.

The rule: every path that explicitly sets or clears `metadata.title` (without
LLM generation) also writes a `ConfigDelta` event with
`conversation.title.generate.auto_refresh.turn_interval = 0`, disabling
auto-refresh for that conversation.
To re-enable later, the user can run `config set
conversation.title.generate.auto_refresh.turn_interval 5` (or any positive
value) to write a new `ConfigDelta` that re-enrolls the conversation.

The affected surfaces:

| Surface                              | Behavior                                |
| ------------------------------------ | --------------------------------------- |
| `conversation edit --title` (no arg) | Regenerates via LLM. Sets               |
|                                      | `title_generated_at_turn = Some(turn)`. |
|                                      | No disable-delta — the user opted into  |
|                                      | LLM-driven titling.                     |
| `conversation edit --title "T"`      | Sets `title = Some("T")`, writes        |
|                                      | disable-delta.                          |
| `conversation edit --no-title`       | Clears title, writes disable-delta.     |
| `query --title "T"`                  | Sets `title = Some("T")`, writes        |
|                                      | disable-delta.                          |
| `query --no-title`                   | Clears title, writes disable-delta.     |
| `conversation fork --title "T"`      | Sets `title = Some("T")` on the fork,   |
|                                      | writes disable-delta on the fork.       |

The disable-delta is a single small helper invoked from each call site rather
than scattered logic.

### Watermark invariants under stream changes

`title_generated_at_turn` is a position into the event stream.
Any operation that changes the position of `turn_start` events relative to the
watermark must update the watermark — otherwise it can drift past `turn_count`,
leaving the conversation permanently ineligible for refresh.

Concretely, this affects forks that retain a tail of the stream:

- `conversation fork --last N` calls `events.retain_last_turns(N)` on the fork's
  events.
  The fork inherits the source's metadata via clone, so a source with
  `title_generated_at_turn = Some(k)` and `k > N` produces a fork whose
  watermark exceeds its own turn count.
- `conversation fork --from/--until` similarly drops events.

**The rule:** any operation that drops `turn_start` events from a stream clamps
the resulting watermark to `min(watermark, new_turn_count)`.
For the common `fork --last 1` (a one-turn snapshot), this lands the watermark
at `1`, matching the semantics of the first-turn auto-generation that runs on a
fresh conversation.

A `conversation fork --title "..."` invocation writes a disable-delta on the
fork (per [Interaction with manual title surfaces][manual-title-surfaces]), so
the watermark on the fork is irrelevant to refresh decisions.

`ConversationStream::retain_last_turns` is the only stream-shortening operation
in the codebase today.
[RFD 064] (non-destructive compaction) does *not* affect the watermark —
compaction events are appended overlays and do not change the underlying
`turn_start` count.

### Background task

A new `TitleRefreshTask` runs the full refresh pipeline in the background:
candidate scanning, turn counting, stream loading, and LLM calls.
The main thread's only responsibility is spawning the task — all heavy I/O
happens off the critical path.

#### Spawn (main thread)

At the start of a `jp query` invocation, after the workspace is loaded, a single
`TitleRefreshTask` is spawned when `turn_interval > 0 && auto &&
ctx.term.args.persist`.
The `persist` gate matches the existing first-turn title spawn at
`crates/jp_cli/src/cmd/query.rs` — under `--no-persist`, writes are no-ops
through `NullPersistBackend` but the LLM call would still cost money, so
auto-refresh is unconditionally suppressed in that mode.

The task receives:

- An `Arc<dyn LoadBackend>` cloned from the workspace, used for read-only
  scanning.
  After [RFD 073], `LoadBackend` is the public trait for reading conversation
  IDs, metadata, and event streams; the underlying `Storage` struct is a private
  implementation detail of `jp_storage` and is not exposed to callers.
  The task uses `LoadBackend::load_conversation_ids`,
  `LoadBackend::load_conversation_metadata`, and
  `LoadBackend::load_conversation_stream`.
- An `Arc<dyn LockBackend>` cloned from the workspace, used for the preflight
  lock check before each LLM call.
- The active conversation ID (to exclude it from candidacy).
- The `AutoRefreshConfig` from configuration.
- Provider and model configuration for the LLM call.

No conversation scanning or event file reading happens on the main thread.

This aligns with [RFD 074]'s direction for a fallible escape-hatch API for
background tasks, but does not depend on it — the trait methods used here exist
today.

#### Run (background)

The task performs the following steps inside `run()`:

1. List all conversation IDs via `LoadBackend::load_conversation_ids`.

2. For each conversation, load metadata via
   `LoadBackend::load_conversation_metadata` to get `title`,
   `title_generated_at_turn`, `last_activated_at`, and `turn_count`.
   
   `load_conversation_metadata` already calls the lightweight
   `load_count_and_timestamp_events` to populate `events_count` and
   `last_event_at` from `events.json`.
   This function is extended to also deserialize the `type` field and count
   `turn_start` events, populating a new `turn_count` field on `Conversation`.
   The extension adds one field to the internal `RawEvent` struct:
   
   ```rust
   #[derive(serde::Deserialize)]
   struct RawEvent {
       timestamp: Box<serde_json::value::RawValue>,
       #[serde(rename = "type")]
       event_type: Box<serde_json::value::RawValue>,
   }
   ```
   
   A `turn_start` event is counted when `event_type.get()` equals
   `"\"turn_start\""`.
   This avoids a second pass over `events.json`.

3. Skip the active conversation (it is being actively worked on).

4. Compute eligibility:
   
   - `title_generated_at_turn = Some(n)`: stale when `turn_count >= n +
     turn_interval`.
   - `title_generated_at_turn = None` and `title = None`: eligible with baseline
     `0` — stale when `turn_count >= turn_interval`.
   - `title_generated_at_turn = None` and `title = Some(_)`: skipped (legacy
     manual title; see [Legacy conversations][legacy-conversations]).

5. Sort stale conversations by `last_activated_at` ascending (least recently
   active first).

6. Take up to `batch_size` candidates.

7. For each candidate, in order:
   
   1. Check `CancellationToken`.
      If cancelled, return immediately with the results accumulated so far (see
      [Cancellation](#cancellation) below).
   2. Preflight the conversation lock via `LockBackend::lock_info(id)`.
      If another session holds the lock, the conversation is being actively
      written — skip the candidate to avoid spending an LLM call on work that
      will be discarded at sync time.
   3. Load the full `ConversationStream` via
      `LoadBackend::load_conversation_stream`.
      Inspect the conversation's merged config via `stream.config()` — if
      `turn_interval` has been overridden to `0` via a `ConfigDelta` (e.g., by
      `conversation edit --title "..."` or any other manual title surface), skip
      this candidate.
   4. Scope the stream to the last `turn_context` turns (if `Some(n)`) and run
      the LLM title generation call wrapped in `select!` against
      `token.cancelled()`.
      On cancellation, abort the in-flight LLM call; on success, store the
      result on `self`.

If a file read fails (e.g., partially written by a concurrent session), the task
logs a warning and moves to the next candidate.

The preflight lock check is an optimization, not a correctness mechanism — a
concurrent session can still acquire the lock between the preflight and the LLM
completion.
Correctness is provided by the sync-phase `try_lock` (see [Sync (main
thread)](#sync-main-thread)).

##### Cancellation

The `Task` trait contract is that `run()` returns `Box<dyn Task>` (the same
task), and `TaskHandler::sync` is then called on the returned task.
If the task does not return promptly after cancellation, `TaskHandler` forces
shutdown via `JoinSet::shutdown()` and `sync()` is never called — dropping any
accumulated results.

To preserve completed candidates across cancellation:

- Each LLM call is wrapped in a `select!` against `token.cancelled()`.
- Per-candidate results are stored on `self` immediately after each LLM call
  completes.
- On cancellation, the loop returns `Ok(self)` with whatever has been collected;
  the in-flight candidate is discarded.

This keeps cancellation latency bounded by the per-iteration check rather than
by the full LLM round-trip, so `TaskHandler`'s 2-second forced-shutdown window
is comfortable even with `batch_size = "all"`.

#### Title retention schema

To avoid unnecessary title churn, the title generation schema is extended with a
`retain_current` field.
The LLM receives the current title in its prompt and can indicate that it is
still adequate:

```json
{
  "retain_current": false,
  "titles": [
    "New title suggestion"
  ]
}
```

The prompt includes:

> The conversation currently has the title: "{current\_title}".
> If this title still accurately describes the conversation, set
> `retain_current` to `true`.
> Only generate new titles if the conversation has meaningfully changed
> direction.

When `retain_current` is `true`, the task advances the `title_generated_at_turn`
checkpoint (recording that the title was evaluated) but leaves `title`
unchanged.
This prevents the same conversation from being re-evaluated on every run while
keeping its perfectly good title.

The `title_schema` and `title_instructions` helpers in `jp_llm::title` are
shared by initial generation (`TitleGeneratorTask`), interactive regeneration
(`conversation edit --title`), and the new refresh path.
Adding `retain_current` unconditionally would let the LLM respond with "keep
current" to the interactive regeneration path — which is the opposite of what
the user asked for.

Both helpers are therefore parameterized by a mode (`TitleMode::{Initial,
Regenerate, Refresh}`).
Only `Refresh` includes the current title in the instructions and the
`retain_current` field in the schema.
`Initial` and `Regenerate` keep their current behavior unchanged.

#### Context window safety

The `turn_context` setting (default 10) provides the first line of defense
against oversized requests: only the most recent N turns are sent to the LLM.
This scoping happens before any token-level checks.

The title generation model may still have a smaller context window than those N
turns require.
The inquiry system already solves this problem: it estimates char-based token
counts and drops older events to fit the model's context window.

The core truncation logic — estimate chars, compare to budget, drop oldest
events, re-sanitize — is extracted from `jp_cli::cmd::query::tool::inquiry`
into a shared utility (in `jp_llm` or `jp_conversation`) that both the inquiry
backend and the title generator can use.
Each caller computes its own overhead (the inquiry system accounts for tools,
attachments, and cache-preserving granularity; the title generator only needs
system prompt and title instructions).

The pipeline for each candidate is: scope to last `turn_context` turns \>
estimate chars \> truncate if over budget \> send to LLM.

#### Sync (main thread)

`sync` runs after the background phase completes (or after cancellation returns
the task with accumulated results) and is given `&mut Workspace`.
For each successfully evaluated candidate:

1. `Workspace::acquire_conversation(id)` to obtain a handle.
2. `Workspace::lock_conversation(handle, None)` — a non-blocking `try_lock`
   that returns `LockResult::AlreadyLocked` on contention.
   If the lock is held, log and skip; another session is currently writing.
3. Update `conversation.title` (unless `retain_current` was `true`).
4. Set `conversation.title_generated_at_turn = Some(turn_count_at_evaluation)`,
   where `turn_count_at_evaluation` is the turn count observed by the background
   task when it read the conversation's events.

Using the count at evaluation time rather than at sync time means the checkpoint
advances by what was true when the decision was made, not by any turns added
during the current session.

`ConversationMut::Drop` (per [RFD 069]) flushes the metadata change while the
flock is still held, so the data reaches disk inside the lock window.

If a candidate fails (LLM error, parse failure), the task logs a warning and
skips it.
Successful candidates are still synced.

### Interaction with conversation locks ([RFD 020])

[RFD 020] is implemented; conversation writes are protected by exclusive file
locks.
The title refresh task interacts with locks at three points:

**Preflight (`run`):** Before each LLM call, the task calls
`LockBackend::lock_info(id)`.
A held lock means another session is mid-write — the candidate is skipped
without paying for an LLM call.
This is an optimization; correctness lives in the sync phase below.

**Read (`run`):** Metadata and event reads go through `LoadBackend` without
acquiring a lock.
If a concurrent session is mid-write and the file is partially serialized, the
JSON parse fails and the task moves on.

**Write (`sync`):** `Workspace::lock_conversation` performs a non-blocking
`try_lock`.
On `LockResult::AlreadyLocked`, the title update is discarded; the conversation
will be retried on the next eligible run.
On `LockResult::Acquired`, the metadata change is written through
`ConversationMut`, which auto-persists on drop while the lock is still held.

This approach avoids blocking CLI exit on lock contention and naturally handles
the common case: stale conversations are by definition idle, so lock contention
on them is rare.

### Spawn location

The `TitleRefreshTask` is spawned in `query.rs`, alongside the existing
first-turn title spawn.
This restricts title refresh to `jp query` — the only command with a meaningful
conversation lifetime and where an LLM call is already expected.
Short-lived commands (`conversation ls`, `conversation edit`, etc.) do not
trigger it.

The spawn condition is `turn_interval > 0 && auto && ctx.term.args.persist`.
The `persist` gate matches the existing first-turn spawn at
`crates/jp_cli/src/cmd/query.rs`.
Under `--no-persist`, writes are no-ops via `NullPersistBackend` but an LLM call
would still incur cost — auto-refresh is unconditionally suppressed in that
mode.

The existing first-turn title spawn in `query.rs` is updated to set
`title_generated_at_turn = Some(1)` once the title write completes, so all new
conversations have a baseline and become eligible for future auto-refresh.

## Drawbacks

Each `jp query` run spawns a background task that loads metadata for every
conversation via `LoadBackend`.
Since `load_conversation_metadata` already reads `events.json` (for
`events_count` and `last_event_at`), the turn counting extension adds no extra
file reads — it piggybacks on the existing lightweight parse.
For workspaces with hundreds of conversations this is nonzero I/O, though it
happens entirely in the background and does not delay the user's query.

The `retain_current` schema adds a small amount of complexity to the title
generation prompt and response handling.
Models may occasionally set `retain_current = false` and produce a title that is
semantically identical to the original, causing cosmetic churn.
This is a minor nuisance, not a correctness issue.

## Alternatives

**Timestamp-based staleness.** Track when the title was generated and refresh if
enough time has elapsed.
Rejected: time is a weaker signal than turns.
A conversation that receives one turn per day and one that receives twenty turns
per hour have the same time-based staleness but very different content drift.

**Use `events_count` as a proxy for turns.** Already computed and readily
available.
Rejected: it's imprecise.
A single turn with heavy tool use generates many events; the threshold would
behave inconsistently across different usage patterns.
Turn count is the right unit.

**Cache `turn_count` in `metadata.json`.** Avoids reading `events.json` during
candidacy checks.
Rejected: this introduces derived state from `events.json` into `metadata.json`,
breaking the convention that all conversation-level behavioral state flows
through the event stream's `ConfigDelta`.
The background task architecture makes this optimization unnecessary — the I/O
happens off the critical path.

**Scan on the main thread, load streams in the background.** Perform candidate
selection synchronously and only push the LLM call to the background.
Rejected: candidate scanning requires reading `metadata.json` for every
conversation and `events.json` for stale candidates.
This forces eager loading of all conversation metadata on the main thread,
changing `jp query` startup from O(1) disk reads (active conversation only) to
O(N).
Moving the entire pipeline to the background keeps startup cost at O(1).

## Non-Goals

This RFD does not change when or how the initial title is generated.
The first-turn behavior is unchanged except for setting
`title_generated_at_turn`.

It does not add any user-visible indication that a title was refreshed in the
background.

## Risks and Open Questions

**Concurrent CLI runs.** Two simultaneous `jp query` invocations could both
spawn a `TitleRefreshTask` that selects the same stale conversation.
The result is two LLM requests producing the same (or a slightly different)
title — no data corruption, just a wasted request.
The `sync`-phase locking ([RFD 020]) prevents concurrent metadata writes; the
second task's `try_lock` fails and the update is discarded.

**Token cost of re-titling long conversations.** The `turn_context` default of
10 bounds the typical cost, but users who set `turn_context = false` (unlimited)
or have very long individual turns may still send large payloads.
The context window truncation utility provides a hard safety net, but the cost
scales with the retained context size.
Worth monitoring once the feature ships.

**Title quality on truncated context.** Both `turn_context` scoping and context
window truncation mean the LLM sees only a suffix of the conversation.
The generated title will reflect recent activity rather than the full arc.
This is an acceptable trade-off — recent activity is usually more relevant to
what the user is currently working on — but users should be aware that titles
may shift focus as the conversation evolves.

## Implementation Plan

### Phase 0: Shared truncation utility (independent)

- Extract the core truncation logic (estimate chars, compare to budget, drop
  oldest events, re-sanitize) from `jp_cli::cmd::query::tool::inquiry` into a
  shared utility in `jp_llm` or `jp_conversation`.
- Update the inquiry backend to use the shared utility.
- Update `TitleGeneratorTask::update_title` to truncate the event stream when
  the title model's context window is smaller than the conversation.

### Phase 1: Configuration (independent)

- Add a `try_some_u32_or_false` helper to `jp_config::assignment`, mirroring the
  existing `try_some_bool_or_from_str` pattern: integer → `Some(n)`, boolean
  `false` → `None`, anything else → error.
- Add `AutoRefreshConfig` (with `turn_interval: usize`, `batch_size: BatchSize`,
  `turn_context: Option<usize>`) as a nested config on `GenerateConfig` in
  `jp_config`.
- Wire through `AssignKeyValue` (using the new helper for `turn_context`),
  `PartialConfigDelta`, and `ToPartial` impls.

### Phase 2: State (independent)

- Add `title_generated_at_turn: Option<usize>` to `Conversation` in
  `jp_conversation`.
- Add `turn_count: usize` (computed, `#[serde(skip)]`) to `Conversation`.
- Extend `load_count_and_timestamp_events` in `jp_storage` to count `turn_start`
  events and populate `turn_count`.
- Make `ConversationStream::retain_last_turns` (and any other stream-shortening
  operations) clamp the conversation's `title_generated_at_turn` to
  `min(current, new_turn_count)`.

### Phase 3: Manual-title disable-deltas (depends on Phase 1)

- Add a small helper that writes `ConfigDelta(auto_refresh.turn_interval = 0)`
  to a conversation's event stream.
- Apply the helper from every manual title surface:
  - `conversation edit --title "..."` (user-provided)
  - `conversation edit --no-title`
  - `query --title "..."`
  - `query --no-title`
  - `conversation fork --title "..."`
- Update `conversation edit --title` (no argument) to set
  `title_generated_at_turn = Some(current_turn_count)` after LLM generation.

### Phase 4: Title retention schema (independent)

- Add `TitleMode::{Initial, Regenerate, Refresh}` to `jp_llm::title` and
  parameterize `title_schema` and `title_instructions` on the mode.
- Only `Refresh` adds the `retain_current` field to the schema and the
  current-title context to the prompt.
- Add a companion function (or extend `extract_titles`) that returns the
  `retain_current` flag alongside the title list.

### Phase 5: Task and spawn (depends on Phase 0, 1, 2, 3, 4)

- Implement `TitleRefreshTask` with the full background pipeline: scan
  conversation IDs via `LoadBackend`, read metadata and turn counts, sort
  candidates by `last_activated_at`, preflight with `LockBackend::lock_info`,
  run the LLM call inside a `select!` against `token.cancelled()`, and
  accumulate results into `self`.
- Implement `sync` using `Workspace::acquire_conversation` + `lock_conversation`
  - `ConversationMut`.
    Skip on `LockResult::AlreadyLocked`.
- Update the existing first-turn title spawn in `query.rs` to set
  `title_generated_at_turn = Some(1)` after the title write completes.
- Spawn `TitleRefreshTask` in `query.rs` when `turn_interval > 0 && auto &&
  ctx.term.args.persist`.

### Phase 6: Tests

Coverage for the high-risk paths, paired with the phases that introduce them:

- Legacy custom title (`title_generated_at_turn = None`, `title = Some(_)`) is
  not auto-refreshed.
- Legacy untitled conversation (`title_generated_at_turn = None`, `title =
  None`) is auto-refreshed with baseline `0`.
- `query --title`, `query --no-title`, `conversation edit --title "..."`,
  `conversation edit --no-title`, and `conversation fork --title "..."` each
  write the disable-delta and prevent future refresh.
- `conversation fork --last N` clamps `title_generated_at_turn` to the new turn
  count.
- `--no-persist` does not spawn `TitleRefreshTask`.
- Cancellation after one completed candidate still syncs that result; an
  in-flight candidate is discarded.
- A locked candidate is skipped at preflight without an LLM call.
- `retain_current = true` advances the watermark without changing `title`.

Phases 0, 1, 2, and 4 can be reviewed and merged independently.
Phase 3 depends on Phase 1.
Phase 5 depends on all earlier phases.
Phase 6 (tests) is paired with each phase as it lands.

[RFD 020]: 020-parallel-conversations.md
[RFD 064]: 064-non-destructive-conversation-compaction.md
[RFD 069]: 069-guard-scoped-persistence-for-conversations.md
[RFD 073]: 073-layered-storage-backend-for-workspaces.md
[RFD 074]: 074-eager-loading-with-command-declared-data-requirements.md
[legacy-conversations]: #legacy-conversations
[manual-title-surfaces]: #interaction-with-manual-title-surfaces
