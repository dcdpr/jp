# RFD 126: Turn Summaries

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-30
- **Extends**: [RFD 097]
- **Summary**: Stores a single-line LLM-generated summary on each `TurnStart`
  event, generated in the background during the next query.

## Summary

Every turn in a conversation carries an optional single-line summary on its
`TurnStart` event, alongside the turn's stable `event_id` ([RFD 097]).
At the start of the next `jp query` invocation, a background task ([RFD 107])
summarizes completed turns from their user request, tool call outcomes, and
assistant response using a configurable fast or local model.
Turn-level interfaces such as interactive conversation editing get a `git rebase
-i`-style `<event_id> <summary>` header for each turn without blocking at the
end of a query, because the summary request overlaps the next turn.

## Terminology

- **Turn summary**: a single-line (~60 character) factual description of what a
  turn asked and accomplished, stored on `TurnStart` for human navigation and
  stream editing.

**Not the same as** a compaction **Summary** ([RFD 064]), which is
multi-paragraph prose attached to a `Compaction` overlay (`SummaryPolicy`) that
replaces a range of turns in the provider-facing `Compacted View`.
A turn summary is display metadata on `TurnStart` and never enters a `Thread`.

## Motivation

As conversations grow across dozens of turns, navigating or editing them at the
turn level becomes difficult.
An interactive stream editor (`jp conversation edit --interactive`) presents a
plan file (`CONVERSATION`) that groups events by turn.
Without turn summaries, the plan file can only show bare turn indices (`# Turn
1`, `# Turn 2`) or a truncated `ChatRequest`.

Neither works well for an interactive rebase workflow:

1. **Bare turn numbers carry no context.** Finding "the turn where we switched
   from `Mutex` to `RwLock`" requires opening event files one by one.
2. **Truncating `ChatRequest` fails on conversational follow-ups.** Many turns
   start with short prompts such as `"continue"`, `"yes, do option 2"`, `"fix
   that warning"`, or a pasted compiler error.
   What the turn actually did depends on the request, the tool calls, and the
   assistant's response together.
3. **Synchronous post-turn summarization hurts inner-loop speed.** Summarizing a
   turn after the assistant finishes streaming requires an extra LLM round-trip,
   adding 0.5 to 3 seconds before `jp query` returns to the shell.

JP already generates conversation titles in the background with a fast or local
model (`conversation.title.generate`).
Turn summaries can use the same approach to summarize completed turns without
delaying CLI exit.

## Design

### Consumer Contract

This RFD provides the storage, accessors, and generation pipeline for turn-level
views; it does not ship a consumer.
An interactive stream editor, for example, can label turns in its plan file like
this:

```txt
# Turn 1 (k3m9x2a): Set up project structure with error handling
000-request.md
001-message.md
002-tool-call-fs_create_file.md
003-tool-result-fs_create_file.md

# Turn 2 (p8q2r4b): Add retry backoff to stream handler
004-config-delta.toml
005-request.md
006-reasoning.md
007-tool-call-fs_read_file.md
008-tool-result-fs_read_file.md
009-message.md
```

A consumer can rely on:

- `Turn::id()` and `Turn::summary()` to read a turn's `EventId` and stored
  summary.
- `Turn::fallback_summary()` (re-exported as `mechanical_turn_summary(&Turn)`)
  as the display fallback when a turn has no stored summary.
  It takes the first non-empty line of the turn's first `ChatRequest`, collapses
  whitespace, and truncates to 72 characters.
- `ConversationStream::set_turn_summary` to normalize and store a summary on a
  `TurnStart` entry.

A consumer that rewrites the stream, such as an editor that reconstructs events
from files, must:

- Preserve the original `TurnStart` entry (`event_id` and `summary`) on every
  unchanged turn.
- Clear `summary` on every modified turn so background catch-up regenerates it
  on the next query (see [Summary Freshness](#summary-freshness)).
- Run any on-demand summarization itself (such as summarizing the latest turn
  before opening `$EDITOR`), fall back to the mechanical summary when generation
  is disabled or fails, and persist the result while holding the conversation
  lock.

### Configuration

Turn summary generation is configured under `conversation.turn_summary`:

```toml
[conversation.turn_summary.generate]
auto = true
batch_size = 5

# Optional. Defaults to conversation.title.generate.model, then assistant.model.
[conversation.turn_summary.generate.model]
id = "ollama/llama3.2:3b"
```

- `auto` (default `true`): whether `jp query` spawns background tasks to
  summarize completed turns that lack a summary.
- `batch_size` (default `5`): maximum number of unsummarized completed turns to
  summarize per `jp query` invocation.
  This bounds token and compute cost when resuming a conversation with many
  unsummarized turns.
- `model` (optional `ModelConfig`): the model and parameters used for turn
  summarization.

#### Model Resolution Chain

Model resolution falls back through three settings so users who already
configure a fast or local model for `conversation.title.generate` do not need to
configure it twice:

1. `conversation.turn_summary.generate.model` (if configured)
2. `conversation.title.generate.model` (if configured)
3. `assistant.model.id` with `ParametersConfig::default()`

As in `jp_llm::title::resolve_model`, an unset `parameters.reasoning` defaults
to `ReasoningEffort::Low` with `exclude = true`.

### Data Model: `TurnStart`

`TurnStart` in `jp_conversation::event::turn` changes from a unit struct to a
struct carrying an optional `summary`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TurnStart {
    /// A single-line summary of the turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}
```

In `events.json`, `TurnStart` serializes with its wrapper `event_id` ([RFD 097])
and `summary` on the same JSON object:

```json
{
  "event_id": "k3m9x2a",
  "type": "turn_start",
  "timestamp": "2026-10-30 10:00:00.0",
  "summary": "Add retry backoff to stream handler"
}
```

When `summary` is `None`, the field is omitted and the JSON matches the existing
`TurnStart` format.
Existing `events.json` files without `summary` deserialize with `summary: None`.

#### Updating `TurnStart` In Place

`ConversationStream` treats provider-visible events as append-only ([RFD 064])
because mutating them invalidates cryptographic thinking signatures, alters
provider prompt caches, and destroys raw history.
`TurnStart` is not provider-visible (`EventKind::is_provider_visible()` is
`false`): it never enters a `Thread` and is never sent to an LLM provider, so
updating its `summary` in place preserves all provider-stream invariants.

Storing `summary` directly on `TurnStart` also fits existing stream storage:

- Each turn's summary lives on the same entry as its `event_id`.
- Turn-slicing operations (`retain_turns`, `retain_last_turns`, `fork`,
  `sanitize`, `normalize_turn_starts`) keep `TurnStart` with its turn, so
  summaries stay or drop with the turn automatically.
- `PersistBackend::write` writes `events.json` atomically via a temp-file rename
  when flushing a `ConversationMut`.

#### Stream and Turn API

`Turn<'a>` (in `jp_conversation::stream::turn_iter`) gains three accessors:

```rust
impl<'a> Turn<'a> {
    /// The `EventId` of this turn's leading `TurnStart` event, if present.
    #[must_use]
    pub fn id(&self) -> Option<&'a EventId>;

    /// The single-line summary on this turn's leading `TurnStart`, if present.
    #[must_use]
    pub fn summary(&self) -> Option<&'a str>;

    /// Fallback summary derived from the first non-empty line of the turn's
    /// first `ChatRequest`.
    #[must_use]
    pub fn fallback_summary(&self) -> Option<String>;
}
```

`ConversationStream` adds two methods that target a turn by its `TurnStart`
`EventId` rather than by positional index:

```rust
impl ConversationStream {
    /// Normalize and store `summary` on the `TurnStart` entry with
    /// `event_id == *turn_id`.
    ///
    /// Returns `true` if a non-blank summary was stored on a matching `TurnStart`.
    pub fn set_turn_summary(&mut self, turn_id: &EventId, summary: impl AsRef<str>) -> bool;

    /// Clear the summary on the `TurnStart` entry with `event_id == *turn_id`.
    pub fn clear_turn_summary(&mut self, turn_id: &EventId) -> bool;
}
```

Targeting by `EventId` ensures a background task never writes a summary to the
wrong turn if earlier turns were trimmed while the task ran; if the target turn
was removed, the setter finds no match and drops the summary.
In addition, `ConversationStream::pop` clears the trailing turn's summary
whenever it removes events from that turn.

#### Summary Freshness

A matching `event_id` does not prove the summary still describes the turn.
Because [RFD 097] preserves `event_id` across edits, a turn's events can change
while its `TurnStart` keeps the same `event_id`:

- **While a summary is in flight.** `Query::run` snapshots the candidate turns
  while holding the conversation lock, then releases the lock before
  `TaskHandler::sync` waits for background tasks (up to [RFD 107]'s drain
  timeout) and reacquires it.
  Another process can edit the conversation during that window.
  To detect concurrent edits, the task hashes each turn's stripped transcript
  (including event IDs) into a *fingerprint*.
  When `sync` reacquires the lock, it recomputes the fingerprint from the
  current stream and drops any summary whose turn changed.
  The skipped turn keeps `summary: None` and is retried on the next query.
- **After a summary is stored.** Catch-up only selects turns with `summary:
  None`, so it does not revisit turns that already have a summary.
  Managed editors clear `summary` on modified turns ([Consumer
  Contract](#consumer-contract)).
  For raw edits via `jp conversation edit --events`, the user must remove the
  `summary` field from a modified turn's `turn_start` entry so the next query
  regenerates it.

### Bounded Turn Transcripts

An accurate summary needs the complete turn: the user's `ChatRequest`, the tool
calls and their outcomes, and the assistant's `ChatResponse::Message`.
Raw coding turns, however, often contain tens of kilobytes of tool output (file
reads, grep matches, diffs), long `ChatResponse::Reasoning` blocks, or large
pasted logs in `ChatRequest`.
Sending raw turns to a small local model overflows its context window or adds
seconds of prompt evaluation.

The summarizer transcribes candidate turns directly from raw events in
`ConversationStream`, ignoring compaction overlays so compacted turns are still
read from their original events.
Each turn is formatted as a compact `<turn id="...">` XML block:

- `ChatResponse::Reasoning`, `TurnStart`, and inquiry events are omitted.
- `ToolCallRequest` renders as `<tool_call name="..."/>` without arguments, and
  `ToolCallResponse` renders as `<tool_result/>` or `<tool_error/>`.
- `ChatRequest` and `ChatResponse` messages longer than `MAX_MESSAGE_CHARS`
  (4,000 characters) are truncated to their first and last halves, separated by
  `[…]`.

This reduces a 50k-token coding turn to a few hundred tokens while keeping the
user prompt, every tool name and outcome, and the assistant's final response.

### Lagged Background Catch-Up

`TaskHandler::sync` ([RFD 107]) runs at the end of `jp_cli::lib::run` and waits
for spawned background tasks to finish their `run` phase before exiting.
`TitleGeneratorTask` hides its latency by spawning **before** `run_turn` and
running concurrently with the assistant stream.
A task spawned *after* `run_turn` has no assistant turn to overlap and blocks
the shell prompt at the end of every query.

To summarize the full turn (request, tool calls, and response) without blocking
at the end of a turn, turn summarization uses **lagged background catch-up**:
Turn $N$ is summarized in the background while Turn $N+1$ streams from the
assistant.

```txt
Turn 1 (jp query "...")
  ├─ Main thread: streams Turn 1 from assistant model
  └─ Exit: Turn 1 persisted with TurnStart { summary: None } (no task to wait for)

Turn 2 (jp query "...")
  ├─ Spawn TurnSummaryTask for completed unsummarized turns (Turn 1)
  ├─ Background: summarizes Turn 1 (request + stripped tools + response)
  ├─ Main thread: streams Turn 2 from assistant model
  └─ Exit (TaskHandler::sync): writes Turn 1's summary to TurnStart
     (waits only if the summary request outlasts Turn 2)
```

When the summary request finishes during Turn $N+1$, it adds no exit latency.
If it outlasts the turn (for example, on a cold local model, a short assistant
reply, or a catch-up batch on an older conversation), `TaskHandler::sync` waits
under [RFD 107]'s drain timeout: it prints `⏱ Finishing background tasks…`
after 1 second and cancels the task after 10 seconds.
The task is not cancelled when the assistant turn ends, because a summary model
slower than the assistant turn would otherwise have every result discarded and
re-request the same batch on every query.

#### Spawn (Main Thread)

In `crates/jp_cli/src/cmd/query.rs`, alongside the existing `TitleGeneratorTask`
spawn (before `setup.flush()` and `inputs.run()`):

1. Check `cfg.conversation.turn_summary.generate.auto && ctx.term.args.persist`.
2. Scan `stream` (which holds all turns before the new turn starts) for
   completed turns whose `TurnStart` has `summary: None`.
3. If any exist, construct `TurnSummaryTask` with a snapshot of `stream` and the
   candidate `event_id`s, and spawn it on `ctx.task_handler`.
   Context-window packing is deferred to `run`, where the summary model's window
   size is known.

#### Run (Background)

`TurnSummaryTask::run` executes concurrently with the active turn:

1. Resolve the summary model (see [Model Resolution
   Chain](#model-resolution-chain)) and its context window.

2. Build each candidate's stripped transcript (see [Bounded Turn
   Transcripts](#bounded-turn-transcripts)).

3. Exclude candidates whose transcript alone exceeds the request budget.

4. Pack the remaining candidates, newest first, until reaching `batch_size`
   turns or the token budget.
   Each turn is included in full or omitted.

5. Request structured JSON output with a `summaries` object keyed by each packed
   turn's `TurnStart` `event_id`:

   ```json
   {
     "summaries": {
       "k3m9x2a": "Add retry backoff to stream handler"
     }
   }
   ```

6. Filter the response to `turn_id`s in the packed set, discarding any
   unrequested IDs, and record each packed turn's fingerprint for `sync`.

7. Run the provider call inside `jp_macro::select!` against the
   `CancellationToken`, returning early if cancelled.

`TurnSummaryTask` does not use `jp_llm::window::truncate_to_fit`.
That helper drops individual events from the front of a stream and clears the
stream if no `ChatRequest` remains.
That works when appending a title prompt to a single truncated stream, but turn
summarization needs whole-turn transcripts so every returned summary corresponds
to a complete turn the model saw.

#### Sync (Main Thread)

In `TurnSummaryTask::sync`, after `Query::run` has released the conversation
lock:

1. Acquire a non-blocking conversation lock via
   `Workspace::acquire_conversation` and `Workspace::lock_conversation`,
   matching `TitleGeneratorTask::sync`.
   On `LockResult::AlreadyLocked`, log and return; the next query retries the
   unsummarized turns.
2. In `conv.update_events`, recompute each target turn's fingerprint from the
   locked stream.
   Skip any summary whose turn was removed or whose fingerprint changed (see
   [Summary Freshness](#summary-freshness)), and store the rest with
   `stream.set_turn_summary(&turn_id, summary)`.
3. Flush `conv`.

## Drawbacks

- **One-turn lag on persisted summaries.** Turn $N$'s `TurnStart.summary`
  remains `None` on disk until Turn $N+1$ runs.
- **A slow summary model can delay exit.** When the summary request outlasts the
  overlapped turn, the CLI waits for it up to [RFD 107]'s drain timeout, most
  often while catching up on an older conversation.
  If routine queries regularly hit the `Finishing background tasks…` prompt, we
  can instead cancel the task when the turn ends and accept retrying unfinished
  batches.
- **Oversized turns stay unsummarized.** Each query re-estimates a turn whose
  stripped transcript alone exceeds the summary model's window and skips it
  without sending a request, leaving the turn on the mechanical fallback.
- **Extra background LLM request per turn.** From Turn 2 onward, each `jp query`
  sends a background request to the summary model.
  Stripping keeps each turn small (~200 to 500 tokens), but the call still
  consumes remote tokens or local compute.
- **`TurnStart` is no longer a unit struct.** Existing constructors and pattern
  matches using bare `TurnStart` must use `TurnStart::default()` or `TurnStart {
  .. }`.

## Alternatives

### 1. Introduce a `TurnEnd` Event

Add an `EventKind::TurnEnd(TurnEnd)` variant appended at the end of each turn to
hold `summary` and future turn-scoped metadata (such as token usage).

Rejected for four reasons:

1. **Split identity.** `TurnStart` already carries the turn's `event_id` ([RFD
   097]).
   Storing `summary` on `TurnEnd` splits the turn's ID and summary across two
   stream entries.
2. **Broken invariants on incomplete and edited turns.** As [RFD 023] noted when
   rejecting `TurnEnd`, turns can be interrupted, crash mid-stream, or be edited
   by hand.
   A turn without a `TurnEnd` still needs to appear in `jp conversation edit
   -i`, and every stream-mutating method (`pop`, `retain`, `sanitize`) would
   have to maintain matching `TurnStart`/`TurnEnd` pairs.
3. **Does not avoid in-place mutation.** Because summaries are generated on the
   next turn or regenerated on demand, `TurnEnd` is already on disk when the
   summary arrives, so storing it still mutates a persisted entry in place.
4. **Token usage is per-response, not per-turn.** A single turn can span
   multiple tool-call cycles, retries, continuations, and inquiry models.
   Usage counters belong on individual provider responses, not on a per-turn
   `TurnEnd` event.

### 2. Summarize Only `ChatRequest` Concurrently During Turn $N$

Spawn the summary task at the start of Turn $N$ with only Turn $N$'s
`ChatRequest`, eliminating the one-turn lag.

Rejected because user prompts in multi-turn coding sessions are often short
follow-ups (`"continue"`, `"do option 2"`, `"fix it"`).
Without the tool calls and assistant response, the summary cannot describe what
the turn accomplished.

### 3. Summarize Turn $N$ After the Turn Completes

Spawn the summary task after `run_turn` completes and wait for it in
`drain_background_tasks`.

Rejected because `TaskHandler::sync` blocks CLI exit: waiting for an LLM call
after the assistant finishes streaming delays the shell prompt by the full
duration of the summary request on every query.
Lagged catch-up waits only when the summary request outlasts the next turn.

### 4. Two-Stage Progressive Summarization

Generate a provisional request-only summary for Turn $N$ concurrently during
Turn $N$, then overwrite it with a full-turn summary during Turn $N+1$.

Not adopted.
While this removes the one-turn lag without a post-turn wait, it summarizes
every turn twice and requires tracking `Provisional` versus `Final` state on
`TurnStart`.
We can revisit this if the missing summary on the latest turn proves noticeable.

### 5. Store Turn Summaries in `metadata.json` or a Sidecar File

Store a `HashMap<EventId, String>` in `metadata.json` or in a separate
annotations file alongside `events.json`.

Rejected because it separates a turn's summary from its events across files and
requires every stream-pruning operation (`fork --last`, `retain_turns`, manual
edits) to garbage-collect orphaned `EventId` keys.

## Non-Goals

- **Replacing compaction summaries ([RFD 064]).** Turn summaries are single-line
  labels for human navigation and never replace events in a `Thread`.
- **Persisting token usage or cost metrics.** Durable usage tracking requires
  per-response identity and is out of scope.
- **Building a consumer.** Interactive conversation editing, including on-demand
  summarization before opening `$EDITOR`, is a separate feature.
  This RFD provides the storage and generation pipeline and defines the
  [Consumer Contract](#consumer-contract).

## Risks and Open Questions

- **Prompt cache isolation.** Unlike `jp_llm::title::generate`, which clones and
  rebases the conversation stream, `TurnSummaryTask` sends a standalone request
  containing only the stripped target turns.
  This keeps the summary request from invalidating or polluting the assistant's
  prompt cache, which should be verified across providers during implementation.
- **Summary quality on stripped tool calls.** `ToolCallPolicy::Strip` keeps tool
  names, call IDs, and success/error status while dropping arguments and
  response bodies.
  If summaries are too vague when the assistant's final `ChatResponse` is terse
  (for example, `"Done."`), switching to `ToolCallPolicy::Strip { request:
  false, response: true }` keeps request arguments such as file paths while
  still stripping large response bodies.
- **Stale summaries after raw edits.** Detecting raw `--events` edits relies on
  the user clearing `summary`.
  Persisting a turn fingerprint on `TurnStart` would detect raw edits
  automatically, but adds an on-disk field solely for an escape hatch.
  We will only add it if stale summaries become a problem in practice.
- **Naming in the ubiquitous language.** [RFD 064] already uses "Summary" for
  compaction summaries.
  Discussion should confirm whether `turn_summary` and `TurnStart.summary` are
  distinct enough or whether `TurnStart.title` (`conversation.title.turn`) is
  clearer.

## Implementation Plan

### Phase 1: `TurnStart` Data Model and Stream Accessors

- Change `TurnStart` in `jp_conversation::event::turn` from a unit struct to
  `TurnStart { pub summary: Option<String> }` with `#[derive(Default)]` and
  `skip_serializing_if = "Option::is_none"`.
- Update `TurnStart` construction and pattern-match sites across the workspace.
- Add `Turn::id(&self) -> Option<&EventId>` and `Turn::summary(&self) ->
  Option<&str>` in `crates/jp_conversation/src/stream/turn_iter.rs`.
- Add `ConversationStream::set_turn_summary(&mut self, turn_id: &EventId,
  summary: String) -> bool`.
- Unit tests for serde round-trip (with and without `summary`), backward
  compatibility with legacy `turn_start` JSON, and `set_turn_summary` by
  `EventId`.

Independent; can be reviewed and merged on its own.

### Phase 2: Configuration

- Add `TurnSummaryConfig` and `TurnSummaryGenerateConfig` (`auto: bool`,
  `batch_size: usize`, `model: Option<ModelConfig>`) under `ConversationConfig`
  in `jp_config`.
- Implement `AssignKeyValue`, `PartialConfigDelta`, `FillDefaults`, and
  `ToPartial`.
- Unit tests for config loading, `--cfg` assignment, and delta generation.

Independent; can be reviewed and merged in parallel with Phase 1.

### Phase 3: Turn Summarizer in `jp_llm`

- Add `jp_llm::turn_summary` (mirroring `jp_llm::title`):
  - `resolve_model` implementing the fallback chain
    (`turn_summary.generate.model` -> `title.generate.model` ->
    `assistant.model` with low/excluded reasoning).
  - Standalone transcript builder: copies candidate turns into a fresh stream
    without the conversation's overlays, applies `ReasoningPolicy::Strip` and
    `ToolCallPolicy::Strip`, and projects.
  - Turn fingerprint over a turn's stripped transcript.
  - Turn-granular packing: size estimate, exclusion of oversized turns, and
    newest-first packing bounded by `batch_size` and the request budget.
  - Structured output schema (`summaries: [{ turn_id, summary }]`) and prompt
    instructions (concise, factual, single line, <= 60 chars), with returned IDs
    filtered to the packed set.
  - Mechanical fallback helper (`mechanical_turn_summary(turn: &Turn) ->
    Option<String>`) extracting and truncating the first line of the turn's
    first `ChatRequest`.
- Unit tests for model resolution, mechanical stripping, schema extraction, and
  mechanical fallback.
  Packing tests cover compacted candidate turns (summarized from raw events),
  oversized turns (excluded without a request), and responses naming unrequested
  IDs (discarded).

Depends on Phase 1 and Phase 2.

### Phase 4: `TurnSummaryTask` and `jp query` Integration

- Implement `TurnSummaryTask` in `jp_task::task` (or `jp_cli`):
  - `run`: calls `jp_llm::turn_summary::generate` inside a `select!` against
    `token.cancelled()`.
  - `sync`: locks the conversation via `Workspace::lock_conversation`, drops
    results whose turn fingerprint changed, writes the rest via
    `ConversationStream::set_turn_summary`, and flushes.
- Spawn `TurnSummaryTask` in `crates/jp_cli/src/cmd/query.rs` with the completed
  turns lacking a summary when `auto && persist`.
- Document in `jp conversation edit --events` help that editing a turn's events
  requires clearing its `summary`.
- Integration tests verifying that Turn 1 is summarized during Turn 2 and
  persisted on `TurnStart`, and that an edit to Turn 1 between spawn and sync
  leaves its summary `None`.

Depends on Phase 3.

## References

- [RFD 020] - Parallel Conversations (conversation locking in `Task::sync`)
- [RFD 023] - Resumable Conversation Turns (prior rejection of `TurnEnd`)
- [RFD 053] - Auto-Refresh Conversation Titles (bounded background batching
  pattern)
- [RFD 064] - Non-Destructive Conversation Compaction (mechanical
  `ReasoningPolicy::Strip` and `ToolCallPolicy::Strip` policies)
- [RFD 097] - Stable Event Identifiers (`event_id` on `TurnStart`)
- [RFD 107] - Background Task Infrastructure (`jp_task::Task` and `TaskHandler`)

[RFD 020]: 020-parallel-conversations.md
[RFD 023]: 023-resumable-conversation-turns.md
[RFD 053]: 053-auto-refresh-conversation-titles.md
[RFD 064]: 064-non-destructive-conversation-compaction.md
[RFD 097]: 097-stable-event-identifiers.md
[RFD 107]: 107-background-task-infrastructure.md

