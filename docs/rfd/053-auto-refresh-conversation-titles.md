# RFD 053: Auto-Refresh Conversation Titles

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-18

## Summary

Conversation titles are generated once after the first turn and never
automatically updated. This RFD adds periodic refresh: when
`conversation.title.generate.auto_refresh.turn_interval` is set to a positive
integer N (default 5), the least recently activated conversations that have
accumulated N new turns since their titles were last generated are re-titled as
background tasks on the next `jp query` run.

## Motivation

A title is generated on the first turn of a new conversation and then frozen.
This works well for short, focused conversations, but longer ones take
unexpected turns and end up with a title that describes only the opening
exchange. The user is left with a list of conversations whose titles no longer
reflect what they contain.

The user can already run `conversation edit --title` to manually regenerate a
title, but this requires noticing the problem and taking action. Periodic
automatic refresh should be transparent.

The fix needs to be careful about cost. Triggering a new LLM request on every
`jp query` invocation for every stale conversation in the workspace would spike
API usage and potentially delay the CLI on exit. This design processes a bounded
number of conversations per run in the background — the same pattern already
used for initial title generation.

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
turn_context = 10   # max turns sent to LLM for re-titling (default = 10)
```

`turn_interval = 0` disables the feature entirely.

`batch_size` controls how many stale conversations are refreshed per `jp query`
invocation. The default of `1` spreads the work across runs. Setting it to
`"all"` refreshes every stale conversation in a single run — useful for catching
up after enabling the feature on a workspace with many long-running
conversations, at the cost of more LLM requests.

`turn_context` limits how many recent turns are sent to the LLM when
re-generating a title. For long conversations, earlier turns are often
irrelevant to what the conversation is currently about. The default of `10`
keeps costs predictable and focuses the title on recent activity. Setting it to
`false` disables the limit and sends the full conversation.

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

When `conversation.title.generate.auto = false`, auto-refresh is also disabled
regardless of `turn_interval`.

### Persisted state: `title_generated_at_turn`

To determine staleness, the system needs to know how many turns existed when the
title was last generated. A new optional field is added to `Conversation` in
`metadata.json`:

```rust
/// Turn count when the title was last auto-generated.
///
/// - `None` — legacy conversation; treated as baseline 0 (auto-enrolls in
///   refresh once it has accumulated `turn_interval` turns).
/// - `Some(n)` — title was auto-generated when the conversation had `n` turns.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub title_generated_at_turn: Option<usize>,
```

This field is a pure watermark, not configuration. It lives in `metadata.json`
alongside `title` because they are tightly coupled — one is the output, the
other is the checkpoint that governs when it is regenerated.

| Value     | Meaning                           | Auto-refresh eligible? |
|-----------|-----------------------------------|------------------------|
| `None`    | Legacy conversation (pre-feature) | Yes, baseline 0        |
| `Some(n)` | Auto-generated at turn `n`        | Yes, baseline `n`      |

### Interaction with `conversation edit --title`

`conversation edit --title` (no argument) regenerates the title via LLM. This
sets `title_generated_at_turn = Some(current_turn_count)`, enrolling the
conversation in future auto-refresh with the current position as baseline.

`conversation edit --title "My Custom Title"` sets a user-provided title and
writes a `ConfigDelta` event with `auto_refresh.turn_interval = 0` to the
conversation's event stream, disabling auto-refresh for that conversation. The
user explicitly chose this title; the system should not overwrite it. To
re-enable auto-refresh later, the user can run `config set
conversation.title.generate.auto_refresh.turn_interval 5` (or any positive
value) to write a new `ConfigDelta` that re-enrolls the conversation.

`conversation edit --no-title` removes the title entirely and writes a
`ConfigDelta` event with `auto_refresh.turn_interval = 0`, disabling
auto-refresh for that conversation. The user explicitly chose to have no title;
the system should not generate one later.

### Background task

A new `TitleRefreshTask` runs the full refresh pipeline in the background:
candidate scanning, turn counting, stream loading, and LLM calls. The main
thread's only responsibility is spawning the task — all heavy I/O happens off
the critical path.

#### Spawn (main thread)

At the start of a `jp query` invocation, after the workspace is loaded, a single
`TitleRefreshTask` is spawned if `turn_interval > 0` and `auto = true`. The task
receives:

- A `Storage` handle for filesystem access. `Storage` encapsulates directory
  structure, dual-root resolution (workspace + user), and file I/O. The task
  uses its existing methods (`load_all_conversation_ids`,
  `load_conversation_metadata`, `load_conversation_stream`) rather than
  performing raw filesystem walks.
- The active conversation ID (to exclude it from candidacy).
- The `AutoRefreshConfig` from configuration.
- Provider and model configuration for the LLM call.

No conversation scanning or event file reading happens on the main thread.

#### Run (background)

The task performs the following steps inside `run()`:

1. List all conversation IDs via `Storage::load_all_conversation_ids`.
2. For each conversation, load metadata via
   `Storage::load_conversation_metadata` to get `title`,
   `title_generated_at_turn`, `last_activated_at`, and `turn_count`.

   `load_conversation_metadata` already calls the lightweight
   `load_count_and_timestamp_events` to populate `events_count` and
   `last_event_at` from `events.json`. This function is extended to also
   deserialize the `type` field and count `turn_start` events, populating a new
   `turn_count` field on `Conversation`. The extension adds one field to the
   internal `RawEvent` struct:

   ```rust
   #[derive(serde::Deserialize)]
   struct RawEvent {
       timestamp: Box<serde_json::value::RawValue>,
       #[serde(rename = "type")]
       event_type: Box<serde_json::value::RawValue>,
   }
   ```

   A `turn_start` event is counted when `event_type.get()` equals
   `"\"turn_start\""`. This avoids a second pass over `events.json`.

3. Skip the active conversation (it is being actively worked on).
4. Compute staleness: a conversation is stale if `turn_count >= baseline +
   turn_interval`, where `baseline` is `title_generated_at_turn.unwrap_or(0)`.
5. Sort stale conversations by `last_activated_at` ascending (least recently
   active first).
6. Take up to `batch_size` candidates.
7. For each candidate, load the full `ConversationStream` via
   `Storage::load_conversation_stream`. Check the conversation's merged config
   via `stream.config()` — if `turn_interval` has been overridden to `0` via a
   `ConfigDelta` (e.g., by `conversation edit --title "..."`), skip this
   candidate. Otherwise, scope the stream to the last `turn_context` turns (if
   configured) and make the LLM title generation call.

If a file read fails (e.g., partially written by a concurrent session), the task
skips that conversation and moves to the next.

The task checks its `CancellationToken` between each candidate. If cancelled
(e.g., the user's query has finished and `TaskHandler` is shutting down), the
task stops immediately — it does not wait for an in-flight LLM response or
process remaining candidates. Any titles already generated by completed
candidates are kept and synced; incomplete work is discarded. This avoids
delaying CLI exit when `batch_size` is large or set to `"all"`.

#### Title retention schema

To avoid unnecessary title churn, the title generation schema is extended with a
`retain_current` field. The LLM receives the current title in its prompt and can
indicate that it is still adequate:

```json
{
  "retain_current": false,
  "titles": [
    "New title suggestion"
  ]
}
```

The prompt includes:

> The conversation currently has the title: "{current_title}". If this title
> still accurately describes the conversation, set `retain_current` to `true`.
> Only generate new titles if the conversation has meaningfully changed
> direction.

When `retain_current` is `true`, the task advances the `title_generated_at_turn`
checkpoint (recording that the title was evaluated) but leaves `title`
unchanged. This prevents the same conversation from being re-evaluated on every
run while keeping its perfectly good title.

#### Context window safety

The `turn_context` setting (default 10) provides the first line of defense
against oversized requests: only the most recent N turns are sent to the LLM.
This scoping happens before any token-level checks.

The title generation model may still have a smaller context window than those N
turns require. The inquiry system already solves this problem: it estimates
char-based token counts and drops older events to fit the model's context
window.

The core truncation logic — estimate chars, compare to budget, drop oldest
events, re-sanitize — is extracted from `jp_cli::cmd::query::tool::inquiry` into
a shared utility (in `jp_llm` or `jp_conversation`) that both the inquiry
backend and the title generator can use. Each caller computes its own overhead
(the inquiry system accounts for tools, attachments, and cache-preserving
granularity; the title generator only needs system prompt and title
instructions).

The pipeline for each candidate is: scope to last `turn_context` turns >
estimate chars > truncate if over budget > send to LLM.

#### Sync (main thread)

After the background task completes, `sync` writes results back to the
workspace. For each successfully refreshed candidate:

1. Update `conversation.title` (unless `retain_current` was `true`).
2. Set `conversation.title_generated_at_turn = Some(turn_count_at_evaluation)`,
   where `turn_count_at_evaluation` is the turn count observed by the background
   task when it read the conversation's events.

Using the count at evaluation time rather than at sync time means the checkpoint
advances by what was true when the decision was made, not by any turns added
during the current session.

If a candidate fails (LLM error, parse failure), the task logs a warning and
skips it. Successful candidates are still synced.

### Interaction with conversation locks ([RFD 020])

Once [RFD 020] lands, conversations are protected by exclusive file locks during
write operations. The title refresh task interacts with locks as follows:

**Read phase (`run`):** The task reads `metadata.json` and `events.json` through
`Storage` without acquiring a lock. Read-only access does not require a lock per
[RFD 020]'s model. If a concurrent session is writing to a conversation and the
file is partially written, the JSON parse fails and the task skips that
conversation.

**Write phase (`sync`):** The task attempts a non-blocking lock (`try_lock`) on
each candidate conversation before writing. If the lock is held by another
session, the title update for that conversation is discarded — the work is lost
but no corruption occurs. The conversation will be retried on the next eligible
run. If the lock is free, the task acquires it, writes the metadata update, and
releases it.

This approach avoids blocking the CLI exit on lock contention and naturally
handles the common case: stale conversations are by definition idle, so lock
contention on them is rare.

### Spawn location

The `TitleRefreshTask` is spawned in `query.rs`, alongside the existing
first-turn title spawn. This restricts title refresh to `jp query` — the only
command with a meaningful conversation lifetime and where an LLM call is already
expected. Short-lived commands (`conversation ls`, `conversation edit`, etc.) do
not trigger it.

The existing first-turn title spawn in `query.rs` is updated to set
`title_generated_at_turn = Some(1)`, so all new conversations have a baseline
and become eligible for future auto-refresh.

## Drawbacks

Each `jp query` run spawns a background task that loads metadata for every
conversation via `Storage`. Since `load_conversation_metadata` already reads
`events.json` (for `events_count` and `last_event_at`), the turn counting
extension adds no extra file reads — it piggybacks on the existing lightweight
parse. For workspaces with hundreds of conversations this is nonzero I/O, though
it happens entirely in the background and does not delay the user's query.

The `retain_current` schema adds a small amount of complexity to the title
generation prompt and response handling. Models may occasionally set
`retain_current = false` and produce a title that is semantically identical to
the original, causing cosmetic churn. This is a minor nuisance, not a
correctness issue.

## Alternatives

**Timestamp-based staleness.** Track when the title was generated and refresh if
enough time has elapsed. Rejected: time is a weaker signal than turns. A
conversation that receives one turn per day and one that receives twenty turns
per hour have the same time-based staleness but very different content drift.

**Use `events_count` as a proxy for turns.** Already computed and readily
available. Rejected: it's imprecise. A single turn with heavy tool use generates
many events; the threshold would behave inconsistently across different usage
patterns. Turn count is the right unit.

**Cache `turn_count` in `metadata.json`.** Avoids reading `events.json` during
candidacy checks. Rejected: this introduces derived state from `events.json`
into `metadata.json`, breaking the convention that all conversation-level
behavioral state flows through the event stream's `ConfigDelta`. The background
task architecture makes this optimization unnecessary — the I/O happens off the
critical path.

**Scan on the main thread, load streams in the background.** Perform candidate
selection synchronously and only push the LLM call to the background. Rejected:
candidate scanning requires reading `metadata.json` for every conversation and
`events.json` for stale candidates. This forces eager loading of all
conversation metadata on the main thread, changing `jp query` startup from O(1)
disk reads (active conversation only) to O(N). Moving the entire pipeline to the
background keeps startup cost at O(1).

## Non-Goals

This RFD does not change when or how the initial title is generated. The
first-turn behavior is unchanged except for setting `title_generated_at_turn`.

It does not add any user-visible indication that a title was refreshed in the
background.

## Risks and Open Questions

**Concurrent CLI runs.** Two simultaneous `jp query` invocations could both
spawn a `TitleRefreshTask` that selects the same stale conversation. The result
is two LLM requests producing the same (or a slightly different) title — no data
corruption, just a wasted request. Once [RFD 020] lands, the `sync`-phase
locking prevents concurrent metadata writes; the second task's `try_lock` fails
and the update is discarded.

**Token cost of re-titling long conversations.** The `turn_context` default of
10 bounds the typical cost, but users who set `turn_context = false` (unlimited)
or have very long individual turns may still send large payloads. The context
window truncation utility provides a hard safety net, but the cost scales with
the retained context size. Worth monitoring once the feature ships.

**Title quality on truncated context.** Both `turn_context` scoping and context
window truncation mean the LLM sees only a suffix of the conversation. The
generated title will reflect recent activity rather than the full arc. This is
an acceptable trade-off — recent activity is usually more relevant to what the
user is currently working on — but users should be aware that titles may shift
focus as the conversation evolves.

## Implementation Plan

### Phase 0: Shared truncation utility (independent)

- Extract the core truncation logic (estimate chars, compare to budget, drop
  oldest events, re-sanitize) from `jp_cli::cmd::query::tool::inquiry` into a
  shared utility in `jp_llm` or `jp_conversation`.
- Update the inquiry backend to use the shared utility.
- Update `TitleGeneratorTask::update_title` to truncate the event stream when
  the title model's context window is smaller than the conversation.

### Phase 1: State (independent)

- Add `title_generated_at_turn: Option<usize>` to `Conversation` in
  `jp_conversation`.
- Add `turn_count: usize` (computed, `#[serde(skip)]`) to `Conversation`.
- Extend `load_count_and_timestamp_events` in `jp_storage` to count `turn_start`
  events and populate `turn_count`.
- Update `conversation edit --title` (no argument) to set
  `title_generated_at_turn = Some(current_turn_count)` after LLM generation.
- Update `conversation edit --title "..."` (user-provided) to write a
  `ConfigDelta` with `auto_refresh.turn_interval = 0`.
- Update `conversation edit --no-title` to write a `ConfigDelta` with
  `auto_refresh.turn_interval = 0`.

### Phase 2: Configuration (independent)

- Add `AutoRefreshConfig` (with `turn_interval`, `batch_size`, `turn_context`)
  as a nested config on `GenerateConfig` in `jp_config`.
- Wire through `AssignKeyValue`, `PartialConfigDelta`, and `ToPartial` impls.

### Phase 3: Title retention schema (independent)

- Extend `title_schema` and `title_instructions` in `jp_llm::title` with the
  `retain_current` field and current-title prompt context.
- Update `extract_titles` (or add a companion function) to handle the
  `retain_current` response.

### Phase 4: Task and spawn (depends on Phase 0, 1, 2, 3)

- Implement `TitleRefreshTask` with the full background pipeline: directory
  walk, metadata reading, turn counting, stream loading, LLM calls, and
  per-candidate sync with `try_lock`.
- Update the existing first-turn title spawn in `query.rs` to set
  `title_generated_at_turn = Some(1)`.
- Spawn `TitleRefreshTask` in `query.rs` when `turn_interval > 0` and `auto =
  true`.

Phases 0–3 can be reviewed and merged independently. Phase 4 depends on all of
them.

[RFD 020]: 020-parallel-conversations.md
