# RFD 023: Resumable Conversation Turns

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-19

## Summary

This RFD introduces incremental persistence of tool call results during
execution, an `IncompleteTurn` type that captures residual events from turns
that were interrupted mid-execution, and a `--continue-turn` flag on `jp query`
that resumes incomplete turns from persisted state.

This RFD depends on [RFD 005] (First-Class Inquiry Events) for persisting
`InquiryRequest` and `InquiryResponse` events in the conversation stream.
Without those events on disk, incomplete turns that involve inquiries cannot be
detected or resumed.

## Motivation

Today, a conversation turn runs to completion or it doesn't. If the process
exits mid-turn — the LLM provider was unreachable after the `ChatRequest` was
written, the user hit Ctrl+C during tool execution, or the process crashed — the
persisted stream is left in an inconsistent state. The `sanitize()` method
patches this up on next load by injecting synthetic error responses for orphaned
tool call requests and removing unpaired inquiry events. The incomplete turn's
real state is lost.

This creates two problems:

**Lost work.** If three tools were running and two completed before the process
died, those results are discarded. On retry, all three tools run again. For
tools with side effects (file modifications, git operations), re-execution may
produce different results or fail.

**No retry path.** When the LLM provider is down and the `ChatRequest` is
already persisted, the user works around this with `jp query -E` (no-edit),
which pops the last `ChatRequest` and re-sends it. This works but is a
workaround — the user needs to know about `-E` and understand why their query
appears to have vanished from the stream.

## Design

### `IncompleteTurn`

When a conversation is loaded from disk, the event stream may contain a turn
that didn't finish. Rather than sanitizing away the evidence, JP splits the
stream at the last complete turn boundary and returns the residual events
separately:

```rust
pub struct IncompleteTurn {
    /// Events belonging to the incomplete turn, starting from TurnStart.
    events: Vec<ConversationEvent>,
}
```

The loading function becomes:

```rust
// In jp_workspace or jp_conversation
fn load_conversation(
    path: &Path,
    base_config: Arc<AppConfig>,
) -> Result<(ConversationStream, Option<IncompleteTurn>)>
```

`ConversationStream` contains only complete turns. All existing consumers —
`Thread::into_parts()`, provider conversion, `conversation fork`, `conversation
grep` — work unchanged. They never see incomplete state.

`IncompleteTurn` is only consumed by the turn loop (via `--continue-turn`) and
by display commands that choose to show it (`conversation print`, `conversation
ls`).

#### Detecting an incomplete turn

A turn is structurally complete when all of the following hold for the events
between a `TurnStart` and the next `TurnStart` (or end-of-stream):

1. It contains at least one `ChatRequest` and one `ChatResponse`.
2. Every `ToolCallRequest` has a matching `ToolCallResponse` (matched by
   `tool_call_id`).
3. If `ToolCallResponse` events exist, at least one `ChatResponse` appears after
   the last `ToolCallResponse` — confirming the LLM processed the tool results.

The detection scans the events from the last `TurnStart` to the end of the
stream and checks these conditions. Events that are not part of the conversation
protocol (e.g., future `TurnStatistics` or `TurnMetadata` events) do not affect
completeness — the check is based on structural properties of the
request/response protocol, not on the identity of the last event.

If these conditions are not met, all events from the last `TurnStart` onward
form the `IncompleteTurn`.

The resume phase is determined by *what's missing* from the structural
conditions:

- No `ChatResponse` after `ChatRequest` → resume at `Streaming` (retry LLM).
- Unmatched `ToolCallRequest`s → resume at `Executing` (run remaining tools).
- All tools matched but no follow-up `ChatResponse` → resume at `Streaming`
  (send tool results to LLM).

#### Relationship to `sanitize()`

`ConversationStream::sanitize()` currently repairs structural invariants on load
— injecting synthetic error responses for orphaned `ToolCallRequest`s, removing
unpaired `InquiryRequest`s and `InquiryResponse`s.

With the `IncompleteTurn` split, `sanitize()` no longer needs to handle the
incomplete tail of the stream — that's the `IncompleteTurn`'s job. But
`sanitize()` still enforces structural invariants on the complete turns:

- `TurnStart` at the start of the stream
- `ChatRequest` follows each `TurnStart`
- All complete turns satisfy the structural completeness conditions above
- Request/response pairs are matched within complete turns

The `IncompleteTurn` split happens first: residual events from the last
(incomplete) turn are extracted. Then `sanitize()` runs on the remaining
`ConversationStream` to enforce the invariants above. What `sanitize()` stops
doing is treating the incomplete tail as corruption — no more injecting
synthetic error responses for the last turn's orphaned tool calls, no more
removing unpaired inquiry events that belong to the incomplete turn.

For `conversation fork` with `--from`/`--until` filters, the same pattern
applies: if the slice point falls mid-turn, fork produces `(ConversationStream,
IncompleteTurn)` where the stream satisfies the structural invariants and the
incomplete turn is the valid residual.

### Incremental Persistence

Today, tool call responses are persisted as a batch. The turn loop collects all
`ToolCallResponse` events in memory, writes them to the stream, and calls
`workspace.persist_active_conversation()` after the entire execution phase
completes. If the process exits mid-batch, completed tool results are lost.

This RFD changes the persistence boundary: each `ToolCallResponse` is written to
the stream and persisted individually as each tool completes.

The stream ordering remains correct. `ToolCallRequest` events are already
persisted during the streaming phase (before tool execution begins). During
execution, `ToolCallResponse` events append after the requests:

```
TurnStart
ChatRequest
ChatResponse (with tool calls)
ToolCallRequest1          ← persisted during streaming
ToolCallRequest2          ← persisted during streaming
ToolCallRequest3          ← persisted during streaming
--- streaming phase persist ---
ToolCallResponse1         ← persisted when tool 1 completes
ToolCallResponse3         ← persisted when tool 3 completes
InquiryRequest (tool 2)   ← persisted when tool 2 hits inquiry
--- process exits ---
```

All requests precede all responses, satisfying provider ordering constraints
(e.g. Gemini requires function calls grouped before function responses).
Responses may arrive out of request order (tool 3 before tool 2), but providers
match responses to requests by `tool_call_id`, not by position.

#### What gets persisted incrementally

| Event              | When persisted                           |
|--------------------|------------------------------------------|
| `ToolCallResponse` | Immediately when the tool completes      |
| `InquiryRequest`   | When the tool returns `NeedsInput` with  |
|                    | user target                              |
| `InquiryResponse`  | When the user (or inquiry backend)       |
|                    | answers                                  |

`ChatResponse` and `ToolCallRequest` events are already persisted during the
streaming phase (end of `TurnPhase::Streaming`). No change there.

#### Persistence mechanism

The existing `workspace.persist_active_conversation()` rewrites the entire
conversation stream to disk. For incremental persistence, we call this same
method after each tool completion. This is simple and correct — the full rewrite
ensures consistency.

The call pattern in the turn loop changes from:

```rust
// Before: persist once after all tools complete
for response in responses {
    stream.push(response);
}
workspace.persist_active_conversation()?;
```

To:

```rust
// After: persist after each tool completes
for response in responses {
    stream.push(response);
    workspace.persist_active_conversation()?;
}
```

The `IncompleteTurn` events are persisted alongside the stream. The workspace
persist method writes the stream (complete turns) followed by the incomplete
turn events to the same file. On load, they are split apart.

If the full-rewrite cost becomes a concern for large conversations, an
append-only optimization can be added later without changing the API.

### Incomplete Turn Prompt

When a conversation has an incomplete turn and the user runs `jp query`
*without* `--continue-turn`, the behavior depends on whether a TTY is available.

#### Interactive mode (TTY)

JP displays context about the incomplete turn and offers an inline select prompt
with context-dependent options:

```
$ jp query
⚠ Conversation has an incomplete turn.

  ⏸ fs_modify_file — waiting for input: "Overwrite existing file?"
  ✓ cargo_check — completed
  ✓ fs_read_file — completed

  (r) Resume incomplete turn
  (d) Discard incomplete turn, start fresh
  (q) Exit
  (?) Help
```

The prompt uses the existing `InlineSelect` infrastructure from `jp_inquire`.

Additional context-dependent options appear based on the incomplete turn's
state:

| Last significant state               | Extra options                        |
|--------------------------------------|--------------------------------------|
| `ChatRequest` (pending LLM response) | `(e) Edit pending query`             |
| `InquiryRequest` (waiting for input) | Shows the pending question inline    |

The `(e)` edit option opens the pending `ChatRequest` content in the user's
editor (same as `jp query -E` today). On save, the edited query replaces the
original and the turn resumes from `Streaming`.

> [!NOTE]
> A future enhancement could add a `(s) Select which tools to execute` option
> when tool calls are pending, letting the user toggle individual tools on/off.
> Deselected tools would receive synthetic skip responses. This is deferred
> until the prompt infrastructure proves itself in this context.

#### Non-interactive mode (no TTY)

Without a TTY, JP falls back to an error with guidance, since interactive
prompts aren't possible:

```
$ jp query --id=abc
Error: Conversation abc has an incomplete turn.
  Waiting for input: fs_modify_file — "Overwrite existing file?"

    jp query --continue-turn --id=abc
                                    Resume the incomplete turn.
    jp query --discard-turn --id=abc
                                    Discard the incomplete turn and start fresh.
```

### `--continue-turn` and `--discard-turn` Flags

```
jp query --continue-turn
jp query --continue-turn --id=<cid>
jp query --discard-turn
jp query --discard-turn --id=<cid>
```

`--continue-turn` and `--discard-turn` are explicit flags for scripting, CI, and
users who prefer flags over interactive prompts. When either flag is passed, the
interactive prompt is skipped entirely.

`--continue-turn` signals "resume an incomplete turn if one exists." If the
target conversation has an incomplete turn, JP resumes it. If there is no
incomplete turn, `--continue-turn` is a no-op and `jp query` proceeds normally
(open the editor, send the query, etc.).

`--discard-turn` drops the `IncompleteTurn` events. The `ConversationStream` is
saved without them. If there is no incomplete turn, `--discard-turn` is a no-op.

#### Relationship to `--no-edit`

Today, `jp query -E` (no-edit) with no query pops the last `ChatRequest` from
the stream and re-sends it. This is used as a retry mechanism when the LLM
provider fails after the `ChatRequest` is persisted.

`--continue-turn` handles this case naturally. An incomplete turn where the last
event is a `ChatRequest` (LLM call never succeeded) is resumed by
`--continue-turn`: the turn loop detects the incomplete state, rebuilds the
thread, and retries the LLM call.

`--no-edit` continues to work as it does today. It's less useful for the retry
case now that `--continue-turn` exists, but its primary purpose ("don't open the
editor, use the query as-is") is unchanged.

### Turn Loop Resumption

When `--continue-turn` is used (or the user selects "Resume" from the
interactive prompt) with an `IncompleteTurn`, the incomplete turn's events are
pushed into the in-memory `ConversationStream` before entering the turn loop.
From that point, the turn loop works the same as it does today — events are in
the stream, `build_thread` reads from the stream, tools execute and persist
results. No special dual-source thread building is needed.

The turn loop skips `TurnPhase::Idle` and enters at the appropriate phase based
on what's missing from the structural completeness conditions (see [Detecting an
incomplete turn](#detecting-an-incomplete-turn)):

| Missing condition                        | Resume at   | What happens                         |
|------------------------------------------|-------------|--------------------------------------|
| No `ChatResponse` after `ChatRequest`    | `Streaming` | Retry the LLM call                   |
| Unmatched `ToolCallRequest`s             | `Executing` | Execute remaining tools              |
| Unanswered `InquiryRequest`              | `Executing` | Prompt user, execute remaining tools |
| All tools matched, no follow-up response | `Streaming` | Send tool results to LLM             |

Determining the resume phase and reconstructing the turn state (pending tool
calls, persisted inquiry responses, etc.) from the incomplete turn's events is a
`jp_cli` concern — `IncompleteTurn` itself lives in `jp_conversation` and only
stores events. The analysis functions live in `jp_cli`, either as free functions
or as a wrapper type:

```rust
// In jp_cli
fn pending_tool_calls(incomplete: &IncompleteTurn) -> Vec<ToolCallRequest>;
fn pending_inquiry(incomplete: &IncompleteTurn) -> Option<&InquiryRequest>;
fn reconstruct_turn_state(incomplete: &IncompleteTurn) -> TurnState;
```

`run_turn_loop` gains an `Option<IncompleteTurn>` parameter. When present, the
events are merged into the stream and the initial phase is determined by the
analysis functions above.

#### Promotion on completion

When the resumed turn completes (satisfies all structural completeness
conditions), the turn is now a complete turn within the stream. The
`IncompleteTurn` was already merged at resume time, so the stream is persisted
as usual. On next load, no `IncompleteTurn` is detected.

### Partial `ChatResponse` Persistence

The existing Ctrl+C → "Stop" handler already persists partial `ChatResponse`
content and marks the turn as complete. This handles the graceful interruption
case during LLM streaming.

For ungraceful exits during streaming (crash, SIGKILL), the `EventBuilder` may
hold partial chunks that were never flushed to the stream. These are lost.
However, any events that *were* flushed (complete `ChatResponse` chunks,
`ToolCallRequest` events) are persisted and appear in the `IncompleteTurn`.

A `ChatResponse` stored from a partial stream marks the turn as complete, even
though the LLM didn't finish generating. This is the same behavior as Ctrl+C ->
Stop today. The user sees what was generated and can choose to continue the
conversation with a follow-up query.

### Display Commands

#### `conversation ls`

Shows incomplete turn state based on the last event in the stream:

| Last event                           | Status                                 |
|--------------------------------------|----------------------------------------|
| `InquiryRequest`                     | `waiting-for-input (tool_name)`        |
| `ToolCallRequest`                    | `interrupted (pending tool execution)` |
| `ToolCallResponse` (turn incomplete) | `interrupted (pending follow-up)`      |
| `ChatRequest` (no `ChatResponse`)    | `interrupted (pending LLM response)`   |
| `ChatResponse` (terminal)            | (no special status)                    |

This uses the existing optimized last-event access — no full stream parse
needed.

#### `conversation print`

Renders the `IncompleteTurn` events after the complete stream, with a visual
marker:

```
[assistant] I'll modify those files for you.

⏳ Incomplete turn (waiting for input)
  ✓ cargo_check — completed
  ✓ fs_read_file — completed
  ⏸ fs_modify_file — waiting for input: "Overwrite existing file?"
```

`conversation print` accepts both `ConversationStream` and
`Option<IncompleteTurn>`.

## Drawbacks

**Turn resumption complexity.** Teaching the turn loop to enter at a non-Idle
phase requires careful handling of state reconstruction. The `TurnCoordinator`,
`ToolCoordinator`, and `TurnState` all need to be initialized from persisted
events rather than from scratch. This is a new code path that existing tests
don't cover.

**Incremental persistence adds disk I/O.** Each tool completion triggers a full
conversation rewrite. For a batch of 10 tool calls, that's 10 writes instead of
one. In practice, conversation files are small (tens to hundreds of KB) and the
OS buffers writes. But it's a change in the I/O profile that could be optimized
later with append-only writes.

**Re-execution of tools with side effects.** If a tool completed and its result
was persisted, but the tool also wrote to the filesystem, the conversation
reflects a state that includes those side effects. On resume, other tools in the
batch execute against the modified filesystem. This is correct behavior (the
side effects happened), but it means resume is not identical to a fresh run.
This is inherent to any system that persists partial progress.

## Alternatives

### Keep batch persistence, rely on sanitize

Status quo. Tool results are persisted as a batch. If the process dies
mid-batch, `sanitize()` injects synthetic error responses and removes orphaned
inquiries on next load. The user re-runs the query.

Rejected because it loses completed work and provides no path to answer pending
inquiries after a process exits.

### Marker file for incomplete state

Write a `.incomplete` marker file when a turn starts, delete it when the turn
completes. `conversation ls` checks for the marker instead of parsing events.

Rejected because it duplicates information already derivable from the event
stream (the last event type), adds a file that can get out of sync, and doesn't
help with the core problem (resuming the turn).

### `TurnEnd` event

Introduce an explicit `TurnEnd` event. Absence of `TurnEnd` after a `TurnStart`
signals an incomplete turn.

Rejected because `TurnStart` already serves as the forward boundary marker
(useful for `--tail`, forking, and the `IncompleteTurn` split). `TurnEnd` is
redundant with "the next `TurnStart` or end-of-stream" and adds an event that
must be written (and handled when missing, which defeats the purpose).

### Implicit resume (no `--continue-turn` flag)

`jp query` with no arguments automatically resumes if there's an incomplete
turn, otherwise opens the editor.

Rejected because the same command having two very different behaviors based on
hidden state is confusing. The interactive prompt approach addresses this by
making the incomplete state visible and letting the user choose, while
`--continue-turn` provides the explicit flag for non-interactive use.

## Non-Goals

- **Background execution.** Detaching queries to run in the background is a
  separate concern. This RFD provides the persistence and resumption foundation
  that background execution would depend on.

- **Live attachment to running processes.** IPC for connecting to a running
  query process is a separate concern. This RFD addresses resumption from
  persisted state after a process has exited.

> [!TIP]
> [RFD 027] addresses both background execution and live re-attachment under a
> single client-server model, building on the incomplete turn persistence
> defined here.

- **`--continue-turn` with a new message.** `jp query --continue-turn "extra
  context"` is not proposed. `--continue-turn` resumes without modification. If
  a user wants to add context, they can `--discard-turn` and re-query.

## Risks and Open Questions

### Tool re-execution correctness

When resuming, some tools in the batch may have already completed. Their
`ToolCallResponse` events are in the `IncompleteTurn`. The remaining tools need
execution. For tools that hit an inquiry, the tool is re-executed with the
user's answer.

One-shot tools that already exited with `NeedsInput` are re-executed from
scratch with accumulated answers (the existing `NeedsInput` retry pattern). This
works because one-shot tools are stateless between executions.

Stateful tools ([RFD 009]) that hit an inquiry while running are a different
case — the process that held the tool handle is dead. On resume, the stateful
tool would need to be re-spawned. This RFD does not address stateful tool
resumption; it's deferred to when [RFD 009] is implemented.

### Concurrent access during incremental persistence

If `jp --no-persist query --id=<cid>` reads a conversation while another process
is incrementally persisting, the reader sees a consistent state as of the last
full write. The conversation lock ([RFD 020]) prevents concurrent write access.

### Interaction with `conversation fork`

Forking a conversation with an incomplete turn should include the
`IncompleteTurn` in the fork. This allows the user to answer a pending inquiry
differently in the fork — a valid workflow when exploring alternative
approaches. Both the original and the fork can be independently resumed with
`--continue-turn`.

Similarly, `conversation fork --until=<event>` that slices mid-turn should
produce a fork with an `IncompleteTurn` rather than sanitizing away the residual
events. The fork is valid — it just has an incomplete turn that can be resumed
or discarded.

### ConfigDelta events in incomplete turns

The `IncompleteTurn` may contain `ConfigDelta` events that were applied during
the incomplete turn. When merging the incomplete turn's events back into the
stream on resume, these deltas must be included so that the turn's configuration
state is correct. The existing `ConversationStream::push` and config delta
handling should work as-is — the events are pushed in order and config merging
applies naturally.

## Implementation Plan

### Phase 1: IncompleteTurn Type and Stream Splitting

Add the `IncompleteTurn` type to `jp_conversation`. Implement the stream loading
logic that splits events at the last complete turn boundary. Return
`(ConversationStream, Option<IncompleteTurn>)` from the load path.

Adjust `sanitize()` to not touch the incomplete tail (which is now split into
`IncompleteTurn` before sanitize runs). Update `conversation fork` to produce
`(ConversationStream, Option<IncompleteTurn>)` when slicing mid-turn.

No behavioral changes to `jp query` — the `IncompleteTurn` is detected but not
acted on.

Can be merged independently.

### Phase 2: Incremental Persistence

Modify the tool execution phase in `run_turn_loop` to persist after each
`ToolCallResponse` is added to the stream. The batch persist at the end of the
execution phase remains as a final consistency checkpoint.

Can be merged independently of Phase 1 (improves crash resilience immediately).

### Phase 3: Interactive Prompt and Turn Loop Resumption

Implement the interactive incomplete-turn prompt using `InlineSelect` from
`jp_inquire`. Add `--continue-turn` and `--discard-turn` flags to `jp query` for
non-interactive use.

Implement the analysis functions in `jp_cli` for determining resume phase and
reconstructing turn state from `IncompleteTurn` events. Modify `run_turn_loop`
to accept `Option<IncompleteTurn>` and enter at the appropriate phase.

Non-interactive fallback: `jp query` without `--continue-turn` on a conversation
with an incomplete turn and no TTY produces a clear error with guidance.

Depends on Phase 1 and Phase 2.

### Phase 4: Display Integration

Update `conversation ls` to show incomplete turn status from last-event
detection. Update `conversation print` to render `IncompleteTurn` events with
visual markers.

Depends on Phase 1.

## References

- [RFD 005: First-Class Inquiry Events][RFD 005] — prerequisite for persisting
  `InquiryRequest`/`InquiryResponse` events that this RFD depends on for
  detecting and resuming incomplete turns.
- [RFD 020: Parallel Conversations][RFD 020] — conversation locks that protect
  concurrent write access during incremental persistence.
- [RFD 009: Stateful Tool Protocol][RFD 009] — stateful tools introduce
  process-bound handles that cannot be resumed after exit; noted as a
  limitation.
- `ConversationStream::sanitize()` (`crates/jp_conversation/src/stream.rs`) —
  existing structural repair logic; scope narrowed by this RFD to only cover
  complete turns.

[RFD 005]: 005-first-class-inquiry-events.md
[RFD 009]: 009-stateful-tool-protocol.md
[RFD 020]: 020-parallel-conversations.md
[RFD 027]: 027-client-server-query-architecture.md
