# RFD 036: Conversation Compaction

- **Status**: Superseded
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-08
- **Superseded by**: [RFD 064]
- **Summary**: Introduces `jp conversation compact` with composable mechanical,
  tool-aware, and LLM summarization strategies for reducing conversation size.

## Summary

This RFD introduces `jp conversation compact`, a command that reduces
conversation size through composable strategies ranging from mechanical
transformations (stripping reasoning and deduplicating tool calls) to
LLM-assisted turn summarization.
It also extends the tool protocol so tools can declare their own compaction
rules, such as when `read_file(1,10)` subsumes an earlier `read_file(2,5)` call.

## Motivation

Long-running conversations degrade LLM performance.
Research shows that when models take a wrong turn early in a conversation, they
rarely recover ([Issue #57]).
Even when a conversation stays on track, a growing context window has four
costs:

1. **Higher cost.** Providers bill for every cached and uncached input token.
   Tool call responses (file contents, grep results, and test output) dominate
   the token count in coding conversations.
2. **Slower responses.** More input tokens increase time-to-first-token.
3. **Lower quality.** Models lose focus in long contexts, where obsolete tool
   results and abandoned tangents mislead later turns.
4. **Context window overflow.** The conversation eventually exceeds the model's
   context window and requests fail.

Without dedicated compaction, users work around context growth by forking the
last turn (`jp conversation fork --last 1`), which discards useful history along
with the noise.
JP needs a way to selectively shrink a conversation while preserving relevant
context.

Earlier RFDs also defer conversation compaction to this design:

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

`jp conversation compact` compacts the active conversation, or the conversation
matching `ID`.
Because compaction is lossy, the command **forks** the conversation by default:
it creates and activates a compacted copy while leaving the original
conversation intact.

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

| Flag                | Default | Description                                              |
| ------------------- | ------- | -------------------------------------------------------- |
| `--strategy <name>` | `auto`  | Compaction strategy to apply (repeatable).               |
| `--keep-last <N>`   | `1`     | Number of recent turns to leave untouched.               |
| `--in-place`        | `false` | Modify the conversation in place instead of forking.     |
| `--dry-run`         | `false` | Show a summary of what would change without applying it. |
| `--no-activate`     | `false` | Do not activate the new conversation (fork mode only).   |

#### The `--compact` Flag on `query`

`jp query` also accepts a `--compact` flag that compacts the conversation before
sending the next prompt:

```sh
jp query --compact "Continue working on the feature"
```

This runs `jp conversation compact` with the `auto` strategy and `--keep-last 1`
before executing the query.

### Strategies

A strategy is a function that transforms a `ConversationStream`.
Strategies are composable: when multiple strategies are specified, JP applies
them from left to right.

#### Mechanical Strategies

Mechanical strategies transform the event stream directly without making LLM
calls.

##### `strip-reasoning`

`strip-reasoning` removes all `ChatResponse::Reasoning` events from the
conversation.
Reasoning tokens are internal to the model's thinking process and are not needed
in subsequent turns.

**Impact:** Moderate token reduction for models that emit extended thinking, and
no reduction for models that do not.

##### `strip-tool-results`

`strip-tool-results` replaces `ToolCallResponse` content with a one-line summary
containing the tool name, a success or error indicator, and the first line of
output.
It preserves the `ToolCallRequest` so the model can see what was attempted,
while discarding the full response body.

Before:

```
ToolCallResponse { id: "1", result: Ok("<5000 chars of file content>") }
```

After:

```
ToolCallResponse { id: "1", result: Ok("[compacted] fs_read_file: success") }
```

**Impact:** High token reduction, because tool responses are typically the
largest events in coding conversations.

##### `dedup-tools`

`dedup-tools` finds tool calls with the same tool name and identical arguments,
keeps only the most recent call, and removes earlier request-response pairs.
For example, if `read_file(path: "src/main.rs")` is called in turns 2 and 7, the
call in turn 2 is removed.

**Impact:** Moderate token reduction, especially in long sessions where the
model repeatedly reads the same files.

##### `strip-attachments`

`strip-attachments` removes attachment content from the system prompt of the
compacted conversation, since attachments are typically relevant only for the
initial query.

**Impact:** Variable token reduction, depending on the size of the attachments.

##### `prune-tools`

`prune-tools` removes tool definitions from the conversation configuration when
no `ToolCallRequest` in the stream references that tool, shrinking the system
prompt.

**Impact:** Low to moderate token reduction, depending on the size of the unused
tool definitions.

#### LLM-Assisted Strategies

LLM-assisted strategies invoke a model to compress context semantically, trading
higher latency and token cost for higher-quality compaction.

##### `summarize`

`summarize` sends the conversation prefix (every turn before the `--keep-last`
boundary) to an LLM and replaces those events with a single
`ChatRequest`/`ChatResponse` summary pair at the start of the conversation.

The summarization prompt:

- Includes the full conversation prefix as context.
- Instructs the model to preserve key decisions, file paths, error resolutions,
  and the current state of the task.
- Requests structured output containing a summary, a list of "active files", and
  a list of "open tasks".

The summarization model is configurable and defaults to a fast, inexpensive
model (such as Haiku or GPT-4o-mini):

```toml
[conversation.compaction]
summarize_model = "anthropic/claude-haiku"
```

**Impact:** High token reduction, since a single summary replaces an arbitrary
number of turns.

##### `classify-tangents`

`classify-tangents` sends the conversation to an LLM to identify turns that are
tangential to the current task, returning a list of turn indices that the user
can review and remove.

This strategy is **interactive**: it presents the classified tangents and asks
the user to confirm removal.
In `--dry-run` mode, it lists the candidate turns without prompting.

**Impact:** Variable token reduction, with the largest gains in conversations
that wandered off-track.

#### The `auto` Strategy

`auto` is the default strategy.
It runs the mechanical strategies in sequence and finishes with `summarize` when
the remaining conversation still exceeds a configurable fraction of the model's
context window:

1. `strip-reasoning`
2. `dedup-tools` (including tool-specific subsumption, described below)
3. `strip-tool-results` (for turns outside `--keep-last`)
4. `prune-tools`
5. `summarize` (only when the remaining conversation exceeds a configurable
   threshold, such as 50% of the model's context window)

### Tool Compaction Hints

Each tool can declare how its calls should be compacted via an optional
`compaction` section in the tool configuration.

#### Configuration

```toml
[conversation.tools.fs_read_file.compaction]
# Strategy for compacting this tool's responses
response = "strip" # "keep" | "strip" | "remove"

# Whether duplicate calls (same args) should be deduplicated
dedup = true
```

The `response` field controls how `ToolCallResponse` content is handled:

- `keep`: Leave the response as-is (the default for most tools).
- `strip`: Replace the response with a short summary (tool name and status).
- `remove`: Remove the entire tool call pair (request and response).

#### Tool-Specific Subsumption

For deduplication beyond exact argument matching, tools can declare a `subsumes`
action.
This extends the `Action` enum (`Run`, `FormatArguments`) with a `Subsumes`
variant:

```rust
pub enum Action {
    Run,
    FormatArguments,
    /// Given two tool calls, determine if the first is subsumed by the second.
    Subsumes,
}
```

When JP invokes `Subsumes`, it passes the `earlier` and `later` argument sets to
the tool:

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

The tool returns whether the later call makes the earlier call obsolete:

```json
{
  "type": "success",
  "content": "true"
}
```

This lets a tool express domain-specific rules, such as `read_file(1,10)`
subsuming `read_file(2,5)` on the same path, without hardcoding that knowledge
in JP.
Tools that do not implement `Subsumes` fall back to exact argument equality for
deduplication.

#### Default Compaction Hints

JP's built-in tools use the following default compaction settings:

| Tool             | `response` | `dedup` | `subsumes`                   |
| ---------------- | ---------- | ------- | ---------------------------- |
| `fs_read_file`   | `strip`    | `true`  | Yes (line range containment) |
| `fs_grep_files`  | `strip`    | `true`  | No                           |
| `fs_list_files`  | `strip`    | `true`  | No                           |
| `cargo_check`    | `strip`    | `true`  | No (each run may differ)     |
| `cargo_test`     | `strip`    | `true`  | No                           |
| `fs_create_file` | `strip`    | `false` | No                           |
| `fs_modify_file` | `strip`    | `false` | No                           |
| `git_diff`       | `strip`    | `true`  | No                           |
| `git_commit`     | `keep`     | `false` | No                           |

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

Each strategy implements `Compactor`.
The `auto` strategy is itself a `Compactor` that delegates to a pipeline of
inner compactors.

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

JP prints `CompactionReport` in `--dry-run` mode and as a summary after
compaction completes.

#### Turn Boundary Handling

Every strategy respects `keep_last` by finding the `TurnStart` event at index
`total_turns - keep_last` and operating only on events before that boundary.

After compaction runs, `ConversationStream::sanitize()` (also used by
`conversation fork`) repairs structural invariants such as orphaned tool call
responses and `TurnStart` ordering.

### Configuration

Global compaction settings live under `[conversation.compaction]`:

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

- **Lossy by design.** Compaction permanently discards events from the target
  conversation.
  Even with forking as the default, users who pass `--in-place` can lose context
  they later need.
  The `--dry-run` flag and clear warnings on `--in-place` mitigate this risk.

- **Summarization quality is model-dependent.** A poor summary can mislead the
  model worse than a long conversation.
  A structured summarization prompt and a configurable summarization model
  mitigate this risk.

- **Tool subsumption adds protocol complexity.** `Subsumes` introduces a new
  action to the tool protocol that most third-party tool authors will not
  implement.
  Exact argument equality handles the common case without tool changes, and JP's
  built-in tools implement `Subsumes` where containment rules apply.

- **Interaction with prompt caching.** Compacting a conversation invalidates any
  cached prompt prefix.
  This cost is acceptable because compaction is an explicit user action rather
  than a mid-turn operation.

## Alternatives

### Fork-only (no in-place compaction)

Always creating a new conversation is simpler, but it forces users who only want
to shrink their current conversation to manage extra conversation copies.
Forking by default while providing `--in-place` balances safety and convenience.

### Automatic compaction on every turn

Compacting transparently when a conversation approaches the context window limit
was rejected because compaction is lossy and should remain an explicit user
decision.
Automatic truncation, as used in the inquiry backend, is a separate mechanism
for avoiding hard request failures.

### Provider-side context caching

Providers such as Anthropic and Google cache prompt prefixes automatically.
Prompt caching lowers input token cost, but it does not reduce latency or
quality degradation from long contexts.
Compaction and prompt caching are complementary.

### Single monolithic compact command

Providing a single fixed compaction operation instead of composable strategies
was rejected because different conversations need different transformations.
A coding conversation with many tool calls benefits from `dedup-tools` and
`strip-tool-results`, whereas a discussion-heavy conversation benefits from
`summarize`.
Composable strategies let users tailor the operation.

## Non-Goals

- **Automatic compaction.** This RFD covers explicit, user-initiated compaction;
  automatic compaction triggered by context window proximity has different
  design constraints.
- **Conversation merging.** Combining two conversations into one is a separate
  feature.
- **Conversation rollback.** Rolling a conversation back to an earlier turn is
  already supported by `jp conversation fork --to`.
- **Token counting accuracy.** Compaction uses the existing character-based
  heuristic for token estimation; per-provider tokenizers are orthogonal.

## Risks and Open Questions

- **Summarization prompt quality.** The summary must preserve essential context
  without reintroducing noise.
  Both the prompt wording and whether the prompt should be user-configurable
  require experimentation during implementation.

- **Turn boundary correctness.** The `keep_last` logic must handle edge cases
  such as single-turn conversations, turns without tool calls, and interrupted
  turns.
  The `fork --last` implementation serves as a reference.

- **Subsumption performance.** Checking all pairs of calls for a given tool is
  O(n²) in the number of calls.
  Because most conversations have fewer than 100 calls per tool, pairwise checks
  should be fast in practice, but performance is worth monitoring.

- **Config delta handling.** `ConversationStream` interleaves `ConfigDelta`
  events with conversation events.
  Removing a conversation event must not remove an adjacent `ConfigDelta` that
  affects later turns.

- **Interaction with the knowledge base.** As noted in the knowledge-base
  design, tool calls that recorded learned subjects may be compacted away.
  It remains an open question whether compaction should detect and preserve
  `learn` tool calls automatically or leave that to the user.

## Implementation Plan

### Phase 1: Mechanical Strategies

1. Define the `Compactor` trait and `CompactionReport` in a new
   `jp_conversation::compact` module.
2. Implement the `StripReasoning`, `StripToolResults`, `DedupTools`, and
   `PruneTools` compactors.
3. Implement the `keep_last` turn boundary logic.
4. Add unit tests for each strategy.
5. Add the `jp conversation compact` CLI command with `--strategy`,
   `--keep-last`, `--in-place`, `--dry-run`, and `--no-activate`.

This phase requires no LLM calls and can be merged independently.

### Phase 2: Tool Compaction Hints

1. Add the `compaction` field (`response` and `dedup`) to `ToolConfig`.
2. Wire compaction hints into `StripToolResults` and `DedupTools`.
3. Add default compaction hints to JP's built-in tool configs.
4. Add configuration tests.

This phase depends on Phase 1 and can be merged independently of Phase 3.

### Phase 3: Tool Subsumption Protocol

1. Add `Action::Subsumes` to `jp_tool`.
2. Implement subsumption dispatch in `DedupTools`, calling the tool binary when
   subsumption is configured and falling back to exact equality otherwise.
3. Implement subsumption logic in `fs_read_file` (line range containment).
4. Add integration tests.

This phase depends on Phase 2.

### Phase 4: LLM-Assisted Strategies

1. Implement the `Summarize` compactor to send the conversation prefix to a
   model and replace it with the summary.
2. Implement the `ClassifyTangents` compactor to send the conversation to a
   model, return tangential turn indices, and prompt the user for confirmation.
3. Add the `conversation.compaction` config section (`model` and
   `summarize_threshold`).
4. Add the `--compact` flag to `jp query`.

This phase depends on Phase 1.

### Phase 5: Auto Strategy

1. Implement the `auto` pipeline that composes mechanical and LLM-assisted
   strategies based on conversation state.
2. Add integration tests for the full pipeline.
3. Tune the `summarize_threshold` default based on real-world testing.

This phase depends on Phases 1 through 4.

## References

- [Issue #57]: Make conversation management more powerful.
- [RFD 011]: System Message Queue (compaction interaction).
- [RFD 034]: Inquiry-Specific Assistant Configuration (defers compaction).
- [Multi-turn degradation paper]: Cited in [Issue #57].

[Issue #57]: https://github.com/dcdpr/jp/issues/57
[Multi-turn degradation paper]: https://arxiv.org/abs/2505.06120
[RFD 011]: 011-system-notification-queue.md
[RFD 034]: 034-inquiry-specific-assistant-configuration.md
[RFD 064]: 064-non-destructive-conversation-compaction.md
