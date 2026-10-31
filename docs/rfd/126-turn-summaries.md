# RFD 126: Turn Summaries

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-30
- **Extends**: [RFD 097]
- **Summary**: Store a single-line LLM-generated summary on each `TurnStart`
  event, generated in the background during the next query.

## Summary

Every turn in a conversation carries an optional single-line summary on its
`TurnStart` event, alongside the turn's stable `event_id` ([RFD 097]).
Summaries are generated from the completed turn (user request, tool call
outcomes, and assistant response) by a background task ([RFD 107]) at the start
of the next `jp query` invocation, using a configurable fast or local model.
This gives turn-level interfaces such as interactive conversation editing a `git
rebase -i`-style `<event_id> <summary>` header for every turn without a
dedicated LLM round-trip at the end of a query: the summary request overlaps the
next turn instead.

## Terminology

- **Turn summary**: a single-line (~60 character) factual description of what a
  single turn asked and accomplished, stored on `TurnStart` for human navigation
  and stream editing.

**Not the same as** a compaction **Summary** ([RFD 064]), which is
multi-paragraph prose attached to a `Compaction` overlay (`SummaryPolicy`) that
replaces a range of turns in the provider-facing `Compacted View`.
A turn summary is display metadata on `TurnStart` and never enters a `Thread`.

## Motivation

As conversations grow across dozens of turns, navigating or editing them at the
turn level becomes difficult.
In an interactive stream editor (`jp conversation edit --interactive`), the user
is presented with a plan file (`CONVERSATION`) grouping events by turn.
Without a summary of each turn, the plan file can only display bare turn indices
(`# Turn 1`, `# Turn 2`) or a mechanical truncation of the user's `ChatRequest`.

Neither is sufficient for a git-style interactive rebase workflow:

1. **Bare turn numbers carry no context.** Finding "the turn where we switched
   from `Mutex` to `RwLock`" requires opening individual event files until the
   right exchange is found.
2. **Truncating `ChatRequest` fails on conversational follow-ups.** In pair
   programming, many turns start with short directives such as `"continue"`,
   `"yes, do option 2"`, `"fix that warning"`, or a pasted compiler error.
   The meaning of the turn lives in the combination of the request, the tools
   invoked, and the assistant's response.
3. **Synchronous post-turn summarization hurts inner-loop speed.** Summarizing
   the full turn after the assistant finishes streaming requires an additional
   LLM round-trip.
   Waiting for that call before returning the shell prompt adds 0.5 to 3 seconds
   of dead time at the end of every `jp query`.

We already generate conversation titles in the background using a cheap or local
model (`conversation.title.generate`).
We need an analogous mechanism for per-turn summaries that captures full-turn
outcomes without delaying CLI exit.

## Design

### Consumer Contract

This RFD does not ship a consumer.
It provides the storage, accessors, and generation pipeline that turn-level
views build on.
An interactive stream editor, for example, could label the turns in its plan
file like this:

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

- `Turn::id()` and `Turn::summary()` to read a turn's identity and its stored
  summary.
- `mechanical_turn_summary(&Turn)` as the display fallback when no summary is
  stored: the first non-empty line of the turn's first `ChatRequest`, collapsed
  to a single line and truncated to 72 characters.
- `ConversationStream::set_turn_summary` to persist a summary it generated
  itself.

A consumer that rewrites the stream, such as an editor that reconstructs events
from files, must:

- Keep the original `TurnStart` entry, `event_id` and `summary` included, for
  every turn whose content it did not change.
- Clear `summary` on every turn whose content it changed, so catch-up
  regenerates it on the next query (see [Summary
  Freshness](#summary-freshness)).
- Own any on-demand generation it offers, such as summarizing the most recent
  turn before opening `$EDITOR`, including the mechanical fallback when
  generation is disabled or fails, and persist the result while holding the
  conversation lock.

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
  process in a single `jp query` invocation, bounding token and compute cost
  when resuming a long legacy conversation.
- `model` (optional `ModelConfig`): the model and parameters used for turn
  summarization.

#### Model Resolution Chain

Most users who configure a fast or local model for `conversation.title.generate`
want the same model for turn summaries.
Model resolution therefore follows a three-step fallback chain:

1. `conversation.turn_summary.generate.model` (if configured)
2. `conversation.title.generate.model` (if configured)
3. `assistant.model.id` with fresh `ParametersConfig::default()`

As in `jp_llm::title::resolve_model`, if the resolved model config leaves
`parameters.reasoning` unset, it defaults to `ReasoningEffort::Low` with
`exclude = true`.

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

When `summary` is `None`, the key is omitted and the serialized form is
byte-for-byte identical to today's `TurnStart`.
Legacy files without `summary` deserialize with `summary: None`.

#### Why `TurnStart` In-Place Update Preserves Stream Invariants

`ConversationStream` treats provider-visible events as append-only ([RFD 064])
because mutating them invalidates cryptographic thinking signatures, alters
provider prompt caches, and destroys raw history.

None of those constraints apply to `TurnStart`:

- `TurnStart` is not provider-visible (`EventKind::is_provider_visible()` is
  `false`).
  It never enters a `Thread` and is never sent to an LLM provider.
- Storing `summary` on `TurnStart` co-locates the turn's summary with the turn's
  `event_id`.
- Every turn-slicing operation (`retain_turns`, `retain_last_turns`, `fork`,
  `sanitize`, `normalize_turn_starts`) already keeps `TurnStart` attached to its
  turn, so summaries are retained or dropped with their turn automatically.
- `PersistBackend::write` already writes `events.json` atomically via a
  temp-file rename when flushing a `ConversationMut`.

#### Stream and Turn API

`Turn<'a>` (in `jp_conversation::stream::turn_iter`) gains two accessors:

```rust
impl<'a> Turn<'a> {
    /// The `EventId` of this turn's leading `TurnStart` event, if present.
    #[must_use]
    pub fn id(&self) -> Option<&EventId>;

    /// The single-line summary on this turn's leading `TurnStart`, if present.
    #[must_use]
    pub fn summary(&self) -> Option<&str>;
}
```

`ConversationStream` gains a targeted setter that addresses the turn by its
`TurnStart` `EventId` rather than by positional turn index:

```rust
impl ConversationStream {
    /// Set the summary on the `TurnStart` entry with `event_id == *turn_id`.
    ///
    /// Returns `true` if a matching `TurnStart` was found and updated.
    pub fn set_turn_summary(&mut self, turn_id: &EventId, summary: String) -> bool;
}
```

Addressing by `EventId` decides *which* `TurnStart` a summary is written to.
If the turn was trimmed or replayed away while the task ran, the setter finds no
match and the summary is dropped.

#### Summary Freshness

A matching `event_id` does not prove the summary still describes the turn.
[RFD 097] keeps an entry's `event_id` when its content is edited, so a turn's
content can change underneath its `TurnStart`.
This happens in two ways:

- **While a summary is in flight.** The task captures its turns while the query
  holds the conversation lock, but `sync` reacquires the lock only after the
  background-task drain, which can wait up to [RFD 107]'s shutdown budget.
  Another process can edit the conversation in between.
  `sync` therefore compares each turn's *fingerprint* (a hash of the stripped
  transcript the model received for that turn, event IDs included) against the
  same fingerprint recomputed from the freshly locked stream, and drops any
  result whose turn no longer matches.
  That turn stays `None` and is retried on the next query.
- **After a summary is stored.** Catch-up only nominates turns whose `summary`
  is `None`, so a stored summary is never revisited.
  Managed edits clear it ([Consumer Contract](#consumer-contract)).
  Raw edits through `jp conversation edit --events` are the user's
  responsibility: removing the `summary` key from a turn's `turn_start` entry
  after changing that turn's content makes catch-up regenerate it.

### What is Summarized: Mechanically Stripped Full Turns

To produce an accurate summary, the summarizer needs the complete turn: the
user's `ChatRequest`, the tools that were called and whether they succeeded, and
the assistant's `ChatResponse::Message`.

However, raw coding turns routinely contain tens of kilobytes of tool output
(file reads, grep matches, diffs) and long `ChatResponse::Reasoning` blocks.
Sending raw turns to a small local model overflows its context window or causes
multi-second prompt-evaluation latency.

Before sending candidate turns to the summary model, they are copied into a
fresh `ConversationStream` holding only those turns, without the conversation's
own compaction or patch overlays.
The existing mechanical compaction policies from [RFD 064] are applied to that
stream as a single overlay and projected:

- `ReasoningPolicy::Strip`: omits all `ChatResponse::Reasoning` events.
- `ToolCallPolicy::Strip { request: true, response: true }`: replaces tool
  request arguments with compact summaries and tool response payloads with
  single-line status markers (`[compacted] fs_modify_file: success`).

This shrinks a 50k-token coding turn down to a few hundred tokens while keeping
the user prompt, every tool name and outcome, and the assistant's final answer.

Leaving the conversation's overlays behind matters: a candidate turn inside an
existing summary compaction would otherwise be replaced by a synthetic turn with
an ephemeral ID, and the model would be asked to label a turn it never saw.
Mechanical stripping keeps every retained entry's ID, so each projected turn
still carries the `event_id` of its raw `TurnStart`.

Stripping does not bound `ChatRequest` or `ChatResponse::Message`.
A pasted compiler log or a long final answer survives it, so a single stripped
turn can still exceed a small model's context window.
[Run (Background)](#run-background) fits the request at turn granularity for
that reason.

### When Summarization Runs: Lagged Background Catch-Up

`TaskHandler::sync` ([RFD 107]) runs at the end of `jp_cli::lib::run` and blocks
process exit while waiting for spawned tasks to finish their `run` phase (up to
the 10-second shutdown budget, displaying `⏱ Finishing background tasks…`
after 1 second).

`TitleGeneratorTask` hides its latency because it is spawned **before**
`run_turn` starts and runs concurrently with the main assistant stream.
A task spawned *after* `run_turn` finishes has nothing to hide behind: it blocks
the user's terminal prompt at the end of every turn.

To get full-turn accuracy (request + tool calls + response) without a post-turn
round-trip, turn summarization uses **lagged background catch-up**: Turn $N$ is
summarized concurrently with Turn $N+1$'s assistant stream.

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

The overlap hides the summary request behind the assistant's turn; it does not
remove it.
When the request outlasts the turn (a cold local model, a short answer, or a
catch-up batch on a legacy conversation), `TaskHandler::sync` waits for it under
[RFD 107]'s budget: up to 10 seconds, then cancellation, with the
finishing-tasks line shown after 1 second.

The task is not cancelled when the turn ends.
With a summary model slower than the user's turns, cancelling at turn end would
discard every result, and each query would request the same batch again.

#### Spawn (Main Thread)

In `crates/jp_cli/src/cmd/query.rs`, alongside the existing `TitleGeneratorTask`
spawn (before `setup.flush()` and `inputs.run()`):

1. Check `cfg.conversation.turn_summary.generate.auto && ctx.term.args.persist`.
2. Scan `stream` (which contains all turns prior to the new turn about to start)
   for completed turns whose `TurnStart` has `summary: None`.
3. If any exist, construct `TurnSummaryTask` with the `stream` snapshot and the
   candidates' `event_id`s, and spawn it on `ctx.task_handler`.
   Choosing which candidates fit happens in `run`, once the summary model's
   context window is known.

#### Run (Background)

`TurnSummaryTask::run` executes concurrently with the active turn:

1. Resolve the summary model (see [Model Resolution
   Chain](#model-resolution-chain)) and its context window.

2. Build each candidate's stripped transcript (see [What is
   Summarized](#what-is-summarized-mechanically-stripped-full-turns)) and
   estimate its size with `jp_llm::window::estimate_chars`.

3. Exclude candidates whose stripped transcript alone exceeds the request
   budget.
   They cost a size estimate and no request, and keep the mechanical display
   fallback.

4. Pack the remaining candidates whole, newest first, until either `batch_size`
   turns or the budget is reached.
   A turn is either in the request entirely or not at all.

5. Request structured JSON output for exactly the packed turns, each tagged with
   its `TurnStart` `event_id`:

   ```json
   {
     "summaries": [
       {
         "turn_id": "k3m9x2a",
         "summary": "Add retry backoff to stream handler"
       }
     ]
   }
   ```

6. Keep only entries whose `turn_id` is in the packed set, discarding anything
   else the model returns, and keep each packed turn's fingerprint for `sync`.

7. Observe the `CancellationToken` via `jp_macro::select!`, returning early if
   cancelled.

`jp_llm::window::truncate_to_fit` is not used.
It drops the oldest events and empties a stream whose `ChatRequest`s did not
survive, which suits a title request appended after fitting, but not a batch in
which every returned label must describe a turn the model actually received.

#### Sync (Main Thread)

In `TurnSummaryTask::sync`, after `Query::run` has released the conversation
lock:

1. Acquire a non-blocking lock on the conversation via
   `Workspace::acquire_conversation` and `Workspace::lock_conversation` (same as
   `TitleGeneratorTask::sync`).
   If `LockResult::AlreadyLocked`, log and skip; the turns will be retried on
   the next run.
2. Call `conv.update_events`.
   For each returned summary, recompute the turn's fingerprint from the locked
   stream and skip the summary if the turn is gone or the fingerprint differs
   (see [Summary Freshness](#summary-freshness)).
   Invoke `stream.set_turn_summary(&turn_id, summary)` for the rest.
3. Flush `conv`.

## Drawbacks

- **One-turn lag on persisted summaries.** Immediately after Turn $N$ finishes,
  Turn $N$'s `TurnStart.summary` on disk is still `None`; it is populated when
  Turn $N+1$ runs.
- **A slow summary model can delay exit.** When the summary request outlasts the
  turn it overlaps, the CLI waits for it under [RFD 107]'s shutdown budget.
  This is most likely while a legacy conversation catches up.
  If routine queries show the finishing-tasks line, the trade-off flips: cancel
  the task when the turn ends and accept the repeated requests.
- **Oversized turns stay unsummarized.** A turn whose stripped transcript alone
  exceeds the summary model's window keeps the mechanical fallback.
  It is re-estimated, never sent, on every query.
- **Additional background LLM request per turn.** From Turn 2 onward, each `jp
  query` makes a background call to the summary model.
  Although mechanical stripping keeps the input small (~200-500 tokens per
  turn), this still spends tokens on remote providers or compute on local
  models.
- **`TurnStart` is no longer a unit struct.** Pattern matches and constructors
  using bare `TurnStart` must be updated to `TurnStart::default()` or `TurnStart
  { .. }`.

## Alternatives

### 1. Introduce a `TurnEnd` Event

Add an `EventKind::TurnEnd(TurnEnd)` variant appended at the end of each turn to
hold `summary` and future turn-scoped metadata (such as token usage).

Rejected for four reasons:

1. **Split identity.** `TurnStart` already carries the turn's `event_id` ([RFD
   097]).
   Putting the summary on `TurnEnd` splits a turn's ID and its summary across
   two separate stream entries.
2. **Broken invariants on incomplete and edited turns.** As [RFD 023] noted when
   rejecting `TurnEnd`, turns can be interrupted, crash mid-stream, or be edited
   by hand.
   A turn without a `TurnEnd` still needs to appear in `jp conversation edit
   -i`, and every stream-mutating method (`pop`, `retain`, `sanitize`) would
   have to maintain matching `TurnStart`/`TurnEnd` pairs.
3. **Does not avoid in-place mutation.** When a summary is generated in a
   background task on the next turn or regenerated on demand, `TurnEnd` has
   already been persisted, so updating the summary still rewrites a historical
   entry.
4. **Token usage is per-response, not per-turn.** A single turn can span
   multiple tool-call cycles, retries, continuations, and inquiry models.
   Usage counters belong on provider-response records rather than a single
   `TurnEnd` event.

### 2. Summarize Only `ChatRequest` Concurrently During Turn $N$

Spawn the summary task at the start of Turn $N$ with only Turn $N$'s
`ChatRequest`, eliminating the one-turn lag.

Rejected because user prompts in multi-turn coding sessions are frequently brief
continuation directives (`"continue"`, `"do option 2"`, `"fix it"`).
Without the tool calls and assistant response, the generated summary cannot
describe what the turn actually did.

### 3. Summarize Full Turn $N$ Post-Turn Before Process Exit

Spawn the summary task after `run_turn` completes and wait for it in
`drain_background_tasks`.

Rejected because `TaskHandler::sync` blocks CLI exit.
Waiting for an LLM call after the assistant's final response has already been
printed delays the return of the shell prompt on every query, by the full
duration of the summary request.
Lagged catch-up waits only for the part of the request that outlasts the next
turn.

### 4. Two-Stage Progressive Summarization

Generate a provisional request-only summary for Turn $N$ concurrently during
Turn $N$, then overwrite it with a full-turn summary during Turn $N+1$.

Not adopted for the initial design.
It eliminates the one-turn lag on disk without a post-turn round-trip, but
requires tracking `Provisional` vs `Final` summary state on `TurnStart` and
summarizes every turn twice.
Can be added later if consumers find the most recent turn's missing summary
noticeable.

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
- **Building a consumer.** Interactive conversation editing, including any
  on-demand generation before opening `$EDITOR`, is a separate feature.
  This RFD provides the storage and generation pipeline and defines the
  [Consumer Contract](#consumer-contract) such an editor must follow.

## Risks and Open Questions

- **Prompt cache isolation.** Unlike `jp_llm::title::generate`, which clones and
  rebases the conversation stream, `TurnSummaryTask` constructs a standalone
  request containing only the mechanically stripped target turns.
  This avoids polluting or invalidating the main assistant's prompt cache, which
  should be verified across providers during implementation.
- **Summary quality on stripped tool calls.** `ToolCallPolicy::Strip` preserves
  tool names, call IDs, and success/error status while dropping response bodies.
  If summaries lack specificity when the assistant's final `ChatResponse` is
  terse (e.g. `"Done."`), `ToolCallPolicy::Strip { request: false, response:
  true }` (keeping tool request arguments such as file paths while stripping
  large response bodies) is a cheap adjustment.
- **Stale summaries after raw edits.** Detection of raw `--events` edits relies
  on the user clearing `summary`.
  Persisting a fingerprint on `TurnStart` would detect them automatically, at
  the cost of an on-disk field that exists only for an escape hatch.
  Not adopted unless stale labels show up in practice.
- **Naming in the ubiquitous language.** "Summary" is already used by [RFD 064]
  for compaction summaries.
  During discussion we should confirm whether `turn_summary` /
  `TurnStart.summary` is clear enough or if `TurnStart.title`
  (`conversation.title.turn`) is preferable.

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

Independent.
Can be reviewed and merged on its own.

### Phase 2: Configuration

- Add `TurnSummaryConfig` and `TurnSummaryGenerateConfig` (`auto: bool`,
  `batch_size: usize`, `model: Option<ModelConfig>`) under `ConversationConfig`
  in `jp_config`.
- Implement `AssignKeyValue`, `PartialConfigDelta`, `FillDefaults`, and
  `ToPartial`.
- Unit tests for config loading, `--cfg` assignment, and delta generation.

Independent.
Can be reviewed and merged in parallel with Phase 1.

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
  Packing tests cover a candidate inside an existing summary compaction (it is
  summarized from its raw events), an oversized turn (excluded without a
  request), and a response naming an unrequested ID (discarded).

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
- Extend the `jp conversation edit --events` long help: clear a turn's `summary`
  after changing its content.
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

