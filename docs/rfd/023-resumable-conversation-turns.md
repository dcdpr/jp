# RFD 023: Resumable Conversation Turns

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-19
- **Required by**: [RFD 027]
- **Summary**: Persists tool results incrementally and splits interrupted turns
  into `IncompleteTurn` state that `--continue-turn` can resume.

## Summary

This RFD introduces incremental persistence of tool call results during
execution, an `IncompleteTurn` type that captures residual events from
interrupted turns, and a `--continue-turn` flag on `jp query` that resumes
incomplete turns from persisted state.

This RFD depends on [RFD 005] (First-Class Inquiry Events), which persists
`InquiryRequest` and `InquiryResponse` events in the conversation stream.
Without those events on disk, JP cannot detect or resume incomplete turns that
involve inquiries.

## Motivation

If the process exits mid-turn (for example, the LLM provider is unreachable
after writing the `ChatRequest`, the user presses Ctrl+C during tool execution,
or the process crashes), the persisted stream is left in an inconsistent state.
On the next load, `sanitize()` repairs the stream by injecting synthetic error
responses for orphaned tool call requests and removing unpaired inquiry events,
discarding the incomplete turn's state.

This creates two problems:

**Lost work.** If three tools are running and two complete before the process
exits, their results are discarded because tool responses are only persisted
after the entire batch finishes.
On retry, all three tools run again.
Re-running tools with side effects (file modifications, git operations) may
produce different results or fail.

**No retry path.** When the LLM provider fails after the `ChatRequest` is
persisted, the user must run `jp query -E` (no-edit) to pop the last
`ChatRequest` and re-send it.
This workaround requires the user to know about `-E` and understand why their
query appears to have vanished from the stream.

## Design

### `IncompleteTurn`

When loading a conversation from disk, the event stream may end with an
unfinished turn.
Rather than sanitizing those events away, JP splits the stream at the last
complete turn boundary and returns the trailing events in an `IncompleteTurn`:

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

`ConversationStream` contains only complete turns, so existing consumers
(`Thread::into_parts()`, provider conversion, `conversation fork`, `conversation
grep`) work unchanged and never see incomplete state.
Only the turn loop (via `--continue-turn`) and display commands (`conversation
print`, `conversation ls`) consume `IncompleteTurn`.

#### Detecting an incomplete turn

A turn is structurally complete when all of the following hold for the events
between a `TurnStart` and the next `TurnStart` (or end-of-stream):

1. It contains at least one `ChatRequest` and one `ChatResponse`.
2. Every `ToolCallRequest` has a matching `ToolCallResponse` (matched by
   `tool_call_id`).
3. If `ToolCallResponse` events exist, at least one `ChatResponse` appears after
   the last `ToolCallResponse`, confirming that the LLM processed the tool
   results.

On load, JP scans the events from the last `TurnStart` to the end of the stream.
If any condition is not met, all events from that `TurnStart` onward form the
`IncompleteTurn`.
Because completeness depends on request/response structure rather than on the
identity of the last event, non-protocol events (such as future `TurnStatistics`
or `TurnMetadata` events) do not affect the check.

Which condition is unmet determines the resume phase:

- No `ChatResponse` after `ChatRequest`: resume at `Streaming` (retry the LLM
  call).
- Unmatched `ToolCallRequest`s: resume at `Executing` (run the remaining tools).
- All tools matched without a follow-up `ChatResponse`: resume at `Streaming`
  (send the tool results to the LLM).

#### Relationship to `sanitize()`

On load, JP extracts any trailing `IncompleteTurn` before running
`ConversationStream::sanitize()`.
Because the incomplete tail is already separated out, `sanitize()` no longer
treats the last turn's orphaned `ToolCallRequest`s or unpaired `InquiryRequest`
and `InquiryResponse` events as corruption.
Instead, `sanitize()` only enforces structural invariants across the completed
turns in `ConversationStream`:

- The stream begins with a `TurnStart`.
- A `ChatRequest` follows each `TurnStart`.
- Every turn satisfies the structural completeness conditions above, with
  matched request/response pairs.

The same split applies to `conversation fork` with `--from`/`--until` filters:
if the slice point falls mid-turn, fork produces `(ConversationStream,
IncompleteTurn)` where the stream satisfies the structural invariants and the
`IncompleteTurn` holds the trailing events.

> [!NOTE]
> `conversation fork` no longer has a slice point that can fall mid-turn.
> Its range flags are `--from`/`--to` (the shared turn selector; `--until` is
> gone), and every bound, including time-based bounds, resolves to a whole turn.
> Range filtering therefore never produces a partial turn, so this behavior
> applies only when forking a conversation whose latest turn is already
> incomplete.
> See [Indexing and Counting Conventions].

### Incremental Persistence

Today, the turn loop collects all `ToolCallResponse` events in memory and calls
`workspace.persist_active_conversation()` only after the entire execution phase
finishes.
If the process exits mid-batch, completed tool results are lost.

With incremental persistence, the turn loop appends each `ToolCallResponse` to
the stream and persists it as soon as the tool completes.

Stream ordering remains valid.
`ToolCallRequest` events are persisted at the end of the streaming phase, before
tool execution begins, so `ToolCallResponse` events always follow the requests:

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

Because all requests precede all responses, the stream satisfies provider
ordering constraints (for example, Gemini requires function calls to be grouped
before function responses).
Responses may arrive out of request order (tool 3 before tool 2), which
providers allow because they match responses to requests by `tool_call_id`
rather than position.

#### What gets persisted incrementally

| Event              | When persisted                            |
| ------------------ | ----------------------------------------- |
| `ToolCallResponse` | When the tool completes                   |
| `InquiryRequest`   | When the tool returns `NeedsInput` with a |
|                    | user target                               |
| `InquiryResponse`  | When the user or inquiry backend answers  |

`ChatResponse` and `ToolCallRequest` events continue to be persisted at the end
of `TurnPhase::Streaming`.

#### Persistence mechanism

`workspace.persist_active_conversation()` rewrites the entire conversation file,
writing the completed turns in `ConversationStream` followed by any
`IncompleteTurn` events (which are split apart again on load).
Incremental persistence calls this same method after each tool completion:

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

A full rewrite keeps the on-disk file consistent without changing the storage
format.
If rewrite cost becomes a bottleneck for large conversations, an append-only
optimization can be added later without changing the API.

### Incomplete Turn Prompt

When a conversation has an incomplete turn and the user runs `jp query` without
`--continue-turn` or `--discard-turn`, the behavior depends on whether a TTY is
available.

#### Interactive mode (TTY)

JP summarizes the incomplete turn and prompts the user with `InlineSelect` from
`jp_inquire`:

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

Additional options appear depending on where the turn stopped:

| Last significant state               | Extra options                     |
| ------------------------------------ | --------------------------------- |
| `ChatRequest` (pending LLM response) | `(e) Edit pending query`          |
| `InquiryRequest` (waiting for input) | Shows the pending question inline |

Selecting `(e)` opens the pending `ChatRequest` in `$EDITOR` (matching `jp query
-E`).
Saving replaces the original query and resumes the turn from `Streaming`.

> [!NOTE]
> A future enhancement could add `(s) Select which tools to execute` when tool
> calls are pending, letting the user toggle individual tools on or off and
> injecting synthetic skip responses for deselected tools.

#### Non-interactive mode (no TTY)

Without a TTY, JP exits with an error that lists the recovery flags:

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

`--continue-turn` and `--discard-turn` bypass the interactive prompt for
scripting, CI, and non-interactive use:

- `--continue-turn` resumes the target conversation's incomplete turn.
  If the conversation has no incomplete turn, the flag is a no-op and `jp query`
  proceeds normally.
- `--discard-turn` drops the `IncompleteTurn` events and persists the
  `ConversationStream` without them.
  If there is no incomplete turn, the flag is a no-op.

#### Relationship to `--no-edit`

Running `jp query -E` (`--no-edit`) without a query pops the last `ChatRequest`
from the stream and re-sends it, which previously served as the retry mechanism
when an LLM call failed after persisting the `ChatRequest`.

`--continue-turn` handles that failure directly: when a turn ends at
`ChatRequest`, the turn loop detects the missing `ChatResponse`, rebuilds the
thread, and retries the LLM call.
`--no-edit` remains unchanged for its primary purpose of sending a query without
opening `$EDITOR`.

### Turn Loop Resumption

When resuming an `IncompleteTurn` (via `--continue-turn` or the interactive
prompt), `run_turn_loop` accepts `Option<IncompleteTurn>`, pushes the incomplete
turn's events back into the in-memory `ConversationStream`, and skips
`TurnPhase::Idle`.
Once the events are in the stream, `build_thread`, tool execution, and
incremental persistence operate on `ConversationStream` as usual without a
separate thread-building path.

The initial `TurnPhase` depends on which structural completeness condition is
unmet (see [Detecting an incomplete turn](#detecting-an-incomplete-turn)):

| Missing condition                        | Resume at   | Action                                      |
| ---------------------------------------- | ----------- | ------------------------------------------- |
| No `ChatResponse` after `ChatRequest`    | `Streaming` | Retry the LLM call                          |
| Unmatched `ToolCallRequest`s             | `Executing` | Execute remaining tools                     |
| Unanswered `InquiryRequest`              | `Executing` | Prompt the user and execute remaining tools |
| All tools matched, no follow-up response | `Streaming` | Send tool results to the LLM                |

`IncompleteTurn` lives in `jp_conversation` and only stores events.
Determining the resume phase and reconstructing turn state (pending tool calls,
persisted inquiry responses) belongs in `jp_cli`:

```rust
// In jp_cli
fn pending_tool_calls(incomplete: &IncompleteTurn) -> Vec<ToolCallRequest>;
fn pending_inquiry(incomplete: &IncompleteTurn) -> Option<&InquiryRequest>;
fn reconstruct_turn_state(incomplete: &IncompleteTurn) -> TurnState;
```

#### Promotion on completion

Because the `IncompleteTurn` events are merged into `ConversationStream` when
resumption starts, finishing the turn requires no separate promotion step.
Once the turn satisfies all structural completeness conditions and is persisted,
the next load sees it as a complete turn.

### Partial `ChatResponse` Persistence

During LLM streaming, the existing Ctrl+C → "Stop" handler flushes the partial
`ChatResponse` to the stream and marks the turn as complete.
A `ChatResponse` persisted from a partial stream satisfies the completeness
check even though generation stopped early, so the user sees the partial output
and can follow up with a new query.

On an ungraceful exit during streaming (such as a crash or `SIGKILL`), any
partial chunks still buffered in `EventBuilder` have not been flushed to the
stream and are lost.
Events already flushed and persisted (complete `ChatResponse` chunks,
`ToolCallRequest` events) remain on disk and are loaded as part of the
`IncompleteTurn`.

### Display Commands

#### `conversation ls`

`conversation ls` reports incomplete turn state from the last event in the
stream, using the existing last-event reader without parsing the full stream:

| Last event                           | Status                                 |
| ------------------------------------ | -------------------------------------- |
| `InquiryRequest`                     | `waiting-for-input (tool_name)`        |
| `ToolCallRequest`                    | `interrupted (pending tool execution)` |
| `ToolCallResponse` (turn incomplete) | `interrupted (pending follow-up)`      |
| `ChatRequest` (no `ChatResponse`)    | `interrupted (pending LLM response)`   |
| `ChatResponse` (terminal)            | (no special status)                    |

#### `conversation print`

`conversation print` accepts both `ConversationStream` and
`Option<IncompleteTurn>`, rendering any incomplete turn after the completed
turns with a status marker:

```
[assistant] I'll modify those files for you.

⏳ Incomplete turn (waiting for input)
  ✓ cargo_check — completed
  ✓ fs_read_file — completed
  ⏸ fs_modify_file — waiting for input: "Overwrite existing file?"
```

## Drawbacks

**Turn resumption complexity.** Entering the turn loop at a non-`Idle` phase
requires initializing `TurnCoordinator`, `ToolCoordinator`, and `TurnState` from
persisted events rather than from scratch, introducing a code path not covered
by existing tests.

**Incremental persistence adds disk I/O.** Each tool completion rewrites the
full conversation file, turning a batch of 10 tool calls from one write into 10.
Conversation files are small (tens to hundreds of KB) and buffered by the OS,
and append-only writes can reduce this overhead later if needed.

**Side effects across resumed tool batches.** When a completed tool modifies the
filesystem before its result is persisted, the remaining tools in the batch run
against that modified filesystem on resume.
Although preserving those side effects is the intended behavior, a resumed turn
does not run against the same initial state as a fresh execution.

## Alternatives

### Keep batch persistence, rely on sanitize

Under the status quo, tool results are persisted as a batch.
If the process exits mid-batch, `sanitize()` injects synthetic error responses
and removes orphaned inquiries on the next load, forcing the user to re-run the
query.
We rejected this because it discards completed tool results and provides no way
to answer pending inquiries after the process exits.

### Marker file for incomplete state

Write a `.incomplete` marker file when a turn starts and delete it when the turn
completes, so `conversation ls` can check for the marker instead of inspecting
events.
We rejected this because the last event in the stream already indicates whether
a turn is incomplete; a separate file can drift out of sync with the stream and
does nothing to support resuming the turn.

### `TurnEnd` event

Write a `TurnEnd` event when a turn finishes, treating any `TurnStart` without a
matching `TurnEnd` as incomplete.
We rejected this because `TurnStart` already marks turn boundaries for `--tail`,
forking, and `IncompleteTurn` splitting.
`TurnEnd` duplicates the boundary already implied by the next `TurnStart` (or
end-of-stream) and introduces another event that must be written and repaired
when missing.

### Implicit resume (no `--continue-turn` flag)

Have `jp query` with no arguments automatically resume an incomplete turn if one
exists, and open `$EDITOR` otherwise.
We rejected this because giving the same command two different behaviors based
on hidden state is surprising.
The interactive prompt surfaces the incomplete turn explicitly and lets the user
choose, while `--continue-turn` provides a deterministic flag for scripts.

## Non-Goals

- **Background execution.** This RFD provides the persistence and resumption
  foundation that detached background queries require, but does not implement
  background execution itself.

- **Live attachment to running processes.** This RFD covers resuming from
  persisted state after a process exits, not IPC for attaching to a running
  query process.

> [!TIP]
> [RFD 027] addresses both background execution and live re-attachment under a
> single client-server model, building on the incomplete turn persistence
> defined here.

- **`--continue-turn` with a new message.** `--continue-turn` resumes the
  existing turn without modification; `jp query --continue-turn "extra context"`
  is not supported.
  To add context, the user can discard the turn with `--discard-turn` and send a
  new query.

## Risks and Open Questions

### Tool re-execution correctness

When a turn resumes during tool execution, tools whose `ToolCallResponse` is
already in `IncompleteTurn` are skipped, and only the remaining tools run.

One-shot tools that exited with `NeedsInput` are stateless between runs, so JP
re-executes them from scratch with the accumulated inquiry answers (the existing
`NeedsInput` retry pattern).

Stateful tools ([RFD 009]) that hit an inquiry while running cannot be resumed
the same way because the process holding the tool handle has exited.
Re-spawning and resuming stateful tools is deferred to [RFD 009].

### Concurrent access during incremental persistence

If `jp --no-persist query --id=<cid>` reads a conversation while another process
is persisting incrementally, the reader sees a consistent snapshot from the most
recent full write.
The conversation lock ([RFD 020]) prevents concurrent writes.

### Interaction with `conversation fork`

Forking a conversation with an incomplete turn copies the `IncompleteTurn` into
the fork.
Both the original conversation and the fork can then be resumed independently
with `--continue-turn`, allowing the user to answer a pending inquiry
differently in each branch.

Similarly, a `conversation fork --until=<event>` command that slices mid-turn
produces a fork with an `IncompleteTurn` rather than sanitizing away the
trailing events, leaving a valid fork whose incomplete turn can be resumed or
discarded.

> [!NOTE]
> This second case is obsolete: fork's range bounds are turn-granular, so no
> fork range can slice mid-turn.
> Forking a conversation that already ends with an incomplete turn remains
> valid.

### ConfigDelta events in incomplete turns

An `IncompleteTurn` may contain `ConfigDelta` events applied before the turn was
interrupted.
Pushing the `IncompleteTurn` events back into `ConversationStream` in order on
resume preserves those deltas and reconstructs the turn's configuration state
via `ConversationStream::push`.

## Implementation Plan

### Phase 1: IncompleteTurn Type and Stream Splitting

Add `IncompleteTurn` to `jp_conversation`, split trailing incomplete events at
the last complete turn boundary on load, and return `(ConversationStream,
Option<IncompleteTurn>)` from the loader.
Run `sanitize()` only on the completed turns in `ConversationStream`, and update
`conversation fork` to return `(ConversationStream, Option<IncompleteTurn>)`
when slicing mid-turn.

`jp query` behavior is unchanged in this phase: `IncompleteTurn` is detected on
load but not yet acted on.
This phase can be merged independently.

### Phase 2: Incremental Persistence

Update the tool execution phase in `run_turn_loop` to persist the conversation
after each `ToolCallResponse` is pushed to the stream, keeping the batch persist
at the end of the execution phase as a final checkpoint.

This phase can be merged independently of Phase 1 and improves crash resilience
on its own.

### Phase 3: Interactive Prompt and Turn Loop Resumption

Implement the interactive incomplete-turn prompt with `InlineSelect` from
`jp_inquire`, add `--continue-turn` and `--discard-turn` to `jp query`, and
return a guided error when `jp query` runs without a TTY on a conversation with
an incomplete turn.
Add the `jp_cli` analysis functions that determine the resume phase and
reconstruct turn state from `IncompleteTurn` events, and update `run_turn_loop`
to accept `Option<IncompleteTurn>` and enter at the corresponding phase.

Depends on Phases 1 and 2.

### Phase 4: Display Integration

Update `conversation ls` to report incomplete turn status from the last event,
and update `conversation print` to render `IncompleteTurn` events with status
markers.

Depends on Phase 1.

## References

- [RFD 005: First-Class Inquiry Events][RFD 005]: persists `InquiryRequest` and
  `InquiryResponse` events, which this RFD requires to detect and resume
  incomplete turns.
- [RFD 020: Parallel Conversations][RFD 020]: defines the conversation locks
  that prevent concurrent writes during incremental persistence.
- [RFD 009: Stateful Tool Protocol][RFD 009]: introduces process-bound tool
  handles that cannot be resumed after process exit.
- `ConversationStream::sanitize()` (`crates/jp_conversation/src/stream.rs`):
  structural repair logic whose scope is narrowed by this RFD to completed
  turns.
- [Indexing and Counting Conventions]: explains why `conversation fork` range
  bounds resolve to whole turns and never slice mid-turn.

[Indexing and Counting Conventions]: ../wiki/00dxyba-indexing-and-counting-conventions.md
[RFD 005]: 005-first-class-inquiry-events.md
[RFD 009]: 009-stateful-tool-protocol.md
[RFD 020]: 020-parallel-conversations.md
[RFD 027]: 027-client-server-query-architecture.md
