# RFD 036: Conversation Compaction

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-08

## Summary

This RFD introduces `jp conversation compact`, a command that reduces
conversation size through composable strategies — from mechanical
transformations (strip reasoning, deduplicate tool calls) to LLM-assisted ones
(summarize older turns). It also extends the tool protocol so that tools can
declare their own compaction rules (e.g., "a `read_file(1,10)` call subsumes an
earlier `read_file(2,5)`").

## Motivation

Long-running conversations degrade LLM performance. Research confirms that when
models take a wrong turn early in a conversation, they don't recover ([Issue
#57]). Even when the conversation stays on track, growing context windows cause:

1. **Higher cost.** Every cached and uncached input token is billed. Tool call
   responses — file contents, grep results, test output — dominate the token
   count in coding conversations.
2. **Slower responses.** More input tokens means higher time-to-first-token.
3. **Lower quality.** Models lose focus in long contexts. Obsolete tool results
   and abandoned tangents actively mislead the model.
4. **Context window overflow.** Eventually the conversation exceeds the model's
   window and fails outright.

Today, users work around this by forking the last turn (`jp conversation fork
--last 1`) and losing all prior context. This is effective but blunt — it
discards useful context along with the noise.

JP needs a way to *selectively* reduce conversation size while preserving the
context that matters.

Multiple existing RFDs defer to this one:
- [RFD 011] (System Message Queue): "If JP ever implements conversation
  compaction..."
- [RFD 034] (Inquiry Config): "smarter compaction (summarization, middle-out
  trimming) is orthogonal"

## Design

### User-Facing Behavior

#### The `compact` Command

```sh
jp conversation compact [ID] [OPTIONS]
```

Compacts the active conversation (or the specified one). By default, the
command **forks** the conversation — it creates a new compacted copy and
activates it, leaving the original intact. This is the safe default because
compaction is lossy.

```sh
# Compact the active conversation (forks by default)
jp conversation compact

# Compact in-place (destructive)
jp conversation compact --in-place

# Compact with a specific strategy
jp conversation compact --strategy strip-reasoning

# Compose multiple strategies (applied left-to-right)
jp conversation compact --strategy strip-reasoning --strategy dedup-tools

# Preview what would change
jp conversation compact --dry-run

# Compact, keeping the last 3 turns intact
jp conversation compact --keep-last 3
```

**Flags:**

| Flag | Default | Description |
|------|---------|-------------|
| `--strategy <name>` | `auto` | Compaction strategy. Repeatable. |
| `--keep-last <N>` | `1` | Number of recent turns to leave untouched. |
| `--in-place` | `false` | Modify the conversation instead of forking. |
| `--dry-run` | `false` | Show a summary of what would change without applying. |
| `--no-activate` | `false` | Don't activate the new conversation (fork mode only). |

#### The `--compact` Flag on `query`

For convenience, `jp query` gains a `--compact` flag that compacts the
conversation before sending the next query:

```sh
jp query --compact "Continue working on the feature"
```

This is equivalent to running `jp conversation compact` followed by
`jp query`, but in a single step. It uses the `auto` strategy with
`--keep-last 1`.

### Strategies

A strategy is a function that transforms a `ConversationStream`. Strategies
are composable — when multiple are specified, they are applied left-to-right.

#### Mechanical Strategies

These are pure transformations that don't require LLM calls.

##### `strip-reasoning`

Removes all `ChatResponse::Reasoning` events from the conversation. Reasoning
tokens are internal to the model's thinking process and are not useful for
continued conversation.

**Impact:** Moderate token reduction for models that emit extended thinking.
Zero reduction for models that don't.

##### `strip-tool-results`

Replaces tool call response content with a short summary: the tool name, a
success/error indicator, and the first line of output. Preserves the tool call
request (so the model knows what was attempted) but discards the full response
body.

Before:
```
ToolCallResponse { id: "1", result: Ok("<5000 chars of file content>") }
```

After:
```
ToolCallResponse { id: "1", result: Ok("[compacted] fs_read_file: success") }
```

**Impact:** High. Tool responses are typically the largest events in coding
conversations.

##### `dedup-tools`

Identifies tool calls with the same name and identical arguments, keeping only
the most recent one. The older call pair (request + response) is removed
entirely.

Example: if `read_file(path: "src/main.rs")` was called at turns 2 and 7,
the turn 2 call is removed.

**Impact:** Moderate. Common in long sessions where the model re-reads files.

##### `strip-attachments`

Removes attachment content from the system prompt of the compacted
conversation. Attachments are typically relevant only for the initial query.

**Impact:** Variable. Depends on attachment size.

##### `prune-tools`

Removes tool definitions from the conversation config that were never used
(no `ToolCallRequest` with that tool name exists in the stream). Reduces the
system prompt size.

**Impact:** Low to moderate. The tool definition list can be large.

#### LLM-Assisted Strategies

These require a model call and are more expensive but produce better results.

##### `summarize`

Sends the older portion of the conversation (everything before the
`--keep-last` boundary) to an LLM with instructions to produce a concise
summary. The summary replaces the original events as a single
`ChatRequest`/`ChatResponse` pair at the start of the conversation.

The summarization prompt:
- Includes the full conversation prefix as context
- Instructs the model to preserve key decisions, file paths, error
  resolutions, and the current state of the task
- Asks for structured output: a summary plus a list of "active files" and
  "open tasks"

The model used for summarization is configurable. Defaults to a fast, cheap
model (e.g., Haiku, GPT-4o-mini) since the task is straightforward.

```toml
[conversation.compaction]
summarize_model = "anthropic/claude-haiku"
```

**Impact:** High. Replaces an arbitrary number of turns with a short summary.

##### `classify-tangents`

Sends the conversation to an LLM and asks it to identify turns that are
tangential to the current task. Returns a list of turn indices that the user
can review and selectively remove.

This strategy is **interactive** — it presents the classified tangents and
asks the user to confirm removal. In `--dry-run` mode, it just lists the
tangents.

**Impact:** Variable. Most useful for conversations that wandered off-track.

#### The `auto` Strategy

`auto` is the default strategy. It composes the mechanical strategies in a
sensible order, with optional LLM-assisted summarization when the conversation
is large enough to warrant it.

The `auto` pipeline:

1. `strip-reasoning`
2. `dedup-tools` (including tool-aware subsumption — see below)
3. `strip-tool-results` (for turns outside `--keep-last`)
4. `prune-tools`
5. `summarize` (only if the remaining conversation exceeds a configurable
   threshold, e.g., 50% of the model's context window)

### Tool Compaction Hints

Tools can declare how their calls should be compacted. This is a new optional
field in the tool configuration.

#### Configuration

```toml
[conversation.tools.fs_read_file.compaction]
# Strategy for compacting this tool's responses
response = "strip" # "keep" | "strip" | "remove"

# Whether duplicate calls (same args) should be deduplicated
dedup = true
```

The `response` field controls what happens to `ToolCallResponse` content:
- `keep`: Leave the response as-is (default for most tools)
- `strip`: Replace with a short summary (tool name + status)
- `remove`: Remove the entire tool call pair (request + response)

#### Tool-Specific Subsumption

For more complex deduplication, tools can declare a `subsumes` action. This
extends the existing `Action` enum (`Run`, `FormatArguments`) with a new
variant:

```rust
pub enum Action {
    Run,
    FormatArguments,
    /// Given two tool calls, determine if the first is subsumed by the second.
    Subsumes,
}
```

When the `Subsumes` action is invoked, the tool receives two sets of arguments
and returns whether the first call is made obsolete by the second:

```json
{
  "tool": {
    "name": "fs_read_file",
    "action": "subsumes",
    "arguments": {
      "earlier": {
        "path": "src/main.rs",
        "start_line": 2,
        "end_line": 5
      },
      "later": {
        "path": "src/main.rs",
        "start_line": 1,
        "end_line": 10
      }
    }
  }
}
```

The tool returns:

```json
{
  "type": "success",
  "content": "true"
}
```

This enables tool-specific logic like "`read_file(2,5)` is subsumed by
`read_file(1,10)` for the same path" without hardcoding that knowledge in JP.

Tools that don't implement the `Subsumes` action fall back to exact argument
equality for deduplication.

#### Default Compaction Hints

JP's built-in tools ship with sensible defaults:

| Tool | `response` | `dedup` | `subsumes` |
|------|-----------|---------|------------|
| `fs_read_file` | `strip` | `true` | Yes (line range containment) |
| `fs_grep_files` | `strip` | `true` | No |
| `fs_list_files` | `strip` | `true` | No |
| `cargo_check` | `strip` | `true` | No (each run may differ) |
| `cargo_test` | `strip` | `true` | No |
| `fs_create_file` | `strip` | `false` | No |
| `fs_modify_file` | `strip` | `false` | No |
| `git_diff` | `strip` | `true` | No |
| `git_commit` | `keep` | `false` | No |

### Internal Architecture

#### The `Compactor` Trait

```rust
/// A compaction strategy that transforms a conversation stream.
pub trait Compactor: Send + Sync {
    /// Apply the compaction strategy to the given stream.
    ///
    /// `keep_last` indicates the number of trailing turns that must not
    /// be modified.
    async fn compact(
        &self,
        stream: &mut ConversationStream,
        keep_last: usize,
    ) -> Result<CompactionReport>;
}
```

Each strategy implements `Compactor`. The `auto` strategy is itself a
`Compactor` that delegates to a pipeline of inner compactors.

#### `CompactionReport`

```rust
pub struct CompactionReport {
    /// Number of events removed.
    pub events_removed: usize,

    /// Estimated tokens saved (char-based heuristic).
    pub estimated_tokens_saved: usize,

    /// Per-strategy breakdown.
    pub steps: Vec<StepReport>,
}

pub struct StepReport {
    pub strategy: String,
    pub events_removed: usize,
    pub estimated_tokens_saved: usize,
}
```

The report is printed in `--dry-run` mode and as a summary after compaction.

#### Turn Boundary Handling

The `keep_last` parameter protects recent turns. All strategies must respect
it. The implementation finds the `TurnStart` event at position
`total_turns - keep_last` and only operates on events before that boundary.

After compaction, `ConversationStream::sanitize()` repairs structural
invariants (orphaned tool call responses, turn start normalization, etc.).
This is the same method already used by `conversation fork`.

### Configuration

```toml
[conversation.compaction]
# Default strategy when --strategy is not specified
strategy = "auto"

# Number of recent turns to preserve
keep_last = 1

# Model to use for LLM-assisted strategies (summarize, classify-tangents)
model = "anthropic/claude-haiku"

# Threshold (fraction of context window) above which auto triggers summarize
summarize_threshold = 0.5

# Whether compact always forks (true) or modifies in-place (false)
fork = true
```

## Drawbacks

- **Lossy by design.** Compaction permanently discards information. Even with
  forking as the default, users may compact in-place and lose context they
  later need. Mitigation: `--dry-run` and clear warnings.

- **Summarization quality is model-dependent.** A poor summary can mislead the
  model worse than a long conversation. Mitigation: the summary prompt is
  carefully designed, and users can choose the summarization model.

- **Tool subsumption adds protocol complexity.** The `Subsumes` action is a new
  tool protocol concept. Most tool authors won't implement it. Mitigation:
  the fallback (exact argument equality) works for the common case, and JP's
  built-in tools ship with subsumption logic.

- **Interaction with prompt caching.** Compacting a conversation invalidates
  any cached prompt prefix. This is acceptable since compaction is an
  explicit user action, not something that happens mid-turn.

## Alternatives

### Fork-only (no in-place compaction)

Always create a new conversation. Simpler, but annoying for users who just want
to slim down their current conversation. The fork-by-default behavior is a
compromise.

### Automatic compaction on every turn

Compact transparently when approaching the context window limit. Rejected:
compaction is lossy and should be an explicit user decision. Automatic
truncation (as in the inquiry backend) is a separate, cruder mechanism for
avoiding hard failures.

### Provider-side context caching

Some providers (Anthropic, Google) cache prompt prefixes automatically. This
reduces cost but doesn't reduce latency or quality degradation from long
contexts. Compaction and caching are complementary.

### Single monolithic compact command

Instead of composable strategies, have a single "compact" operation that does
everything. Rejected: different conversations need different compaction. A
coding conversation with many tool calls benefits from `dedup-tools` +
`strip-tool-results`. A discussion-heavy conversation benefits from
`summarize`. Composability lets users tailor the operation.

## Non-Goals

- **Automatic compaction.** This RFD covers explicit, user-initiated
  compaction. Automatic compaction (triggered by context window proximity) is
  a separate concern with different design constraints.
- **Conversation merging.** Combining two conversations into one. Related but
  distinct.
- **Conversation rollback.** Undoing specific turns. The `fork` command with
  `--until` already covers this.
- **Token counting accuracy.** This RFD uses the existing char-based heuristic
  for token estimation. Accurate token counting (per-provider tokenizer) is
  orthogonal.

## Risks and Open Questions

- **Summarization prompt quality.** The summary needs to preserve the right
  context. What should the prompt look like? Should it be configurable? This
  needs experimentation during implementation.

- **Turn boundary correctness.** The `keep_last` logic must correctly handle
  edge cases: conversations with only 1 turn, turns with no tool calls,
  interrupted turns. The existing `fork --last` implementation is a good
  reference.

- **Subsumption performance.** For tools with many calls, checking all pairs
  for subsumption could be expensive. An O(n²) check per tool name is likely
  fine in practice (most conversations have <100 calls per tool), but worth
  monitoring.

- **Config delta handling.** `ConversationStream` interleaves `ConfigDelta`
  events with conversation events. Compaction must preserve config deltas
  correctly — removing an event shouldn't remove an adjacent config delta that
  affects later events.

- **Interaction with the knowledge base.** As [RFD 008] notes, subjects
  learned via tool calls may be compacted away. Should compaction detect
  `learn` tool calls and preserve them? Or is this the user's responsibility?

## Implementation Plan

### Phase 1: Mechanical Strategies

1. Define the `Compactor` trait and `CompactionReport` in a new
   `jp_conversation::compact` module.
2. Implement `StripReasoning`, `StripToolResults`, `DedupTools`,
   `PruneTools` compactors.
3. Implement the `keep_last` turn boundary logic.
4. Add unit tests for each strategy.
5. Add the `jp conversation compact` CLI command with `--strategy`,
   `--keep-last`, `--in-place`, `--dry-run`, `--no-activate`.

Can be merged independently. No LLM calls required.

### Phase 2: Tool Compaction Hints

1. Add `compaction` field to `ToolConfig` (`response`, `dedup`).
2. Wire compaction hints into `StripToolResults` and `DedupTools`.
3. Add default compaction hints to JP's built-in tool configs.
4. Add config tests.

Depends on Phase 1. Can be merged independently from Phase 3.

### Phase 3: Tool Subsumption Protocol

1. Add `Action::Subsumes` to `jp_tool`.
2. Implement subsumption dispatch in `DedupTools` — call the tool binary
   when subsumption is configured, fall back to exact equality otherwise.
3. Implement subsumption logic in `fs_read_file` (line range containment).
4. Add integration tests.

Depends on Phase 2.

### Phase 4: LLM-Assisted Strategies

1. Implement `Summarize` compactor — sends conversation prefix to a model,
   replaces it with the summary.
2. Implement `ClassifyTangents` compactor — sends conversation to a model,
   returns turn indices, prompts user for confirmation.
3. Add `conversation.compaction` config section (`model`,
   `summarize_threshold`).
4. Add the `--compact` flag to `jp query`.

Depends on Phase 1.

### Phase 5: Auto Strategy

1. Implement the `auto` pipeline that composes mechanical and LLM-assisted
   strategies based on conversation state.
2. Add integration tests for the full pipeline.
3. Tune the `summarize_threshold` default based on real-world testing.

Depends on Phases 1-4.

## References

- [Issue #57] — Make conversation management more powerful
- [RFD 011] — System Message Queue (compaction interaction)
- [RFD 034] — Inquiry-Specific Assistant Configuration (defers compaction)
- [Multi-turn degradation paper](https://arxiv.org/abs/2505.06120) — cited in
  Issue #57

[Issue #57]: https://github.com/dcdpr/jp/issues/57
[RFD 011]: 011-system-notification-queue.md
[RFD 034]: 034-inquiry-specific-assistant-configuration.md
