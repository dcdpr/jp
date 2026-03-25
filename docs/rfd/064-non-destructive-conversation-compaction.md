# RFD 064: Non-Destructive Conversation Compaction

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-17
- **Supersedes**: [RFD 036](036-conversation-compaction.md)

## Summary

This RFD introduces conversation compaction as a non-destructive, additive
operation. Instead of mutating or deleting events in the stored conversation,
compaction appends overlay events that instruct the projection layer to present
a reduced view when building the LLM request. The original events are always
preserved. Compaction policies are defined per content type (summary, reasoning,
tool calls), composed across multiple compaction events, and configured at the
workspace and conversation level.

## Motivation

Long-running conversations degrade LLM performance. Research confirms that when
models take a wrong turn early in a conversation, they don't recover (see:
[Issue #57]). Even when the conversation stays on track, growing context windows
cause:

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
context that matters. Multiple existing RFDs defer to this one:

- [RFD 011] (System Notification Queue): "If JP ever implements conversation
  compaction..."
- [RFD 034] (Inquiry Config): "smarter compaction (summarization, middle-out
  trimming) is orthogonal"

[RFD 036] proposed compaction as a destructive transformation of the
`ConversationStream` — strategies that mutate or replace events, with
fork-by-default as a safety net. This RFD supersedes that design with a
non-destructive approach: compaction events are appended to the stream and
define a *projection* of the original events, preserving the full history while
presenting a reduced view to the LLM.

## Design

### Core Concept: Compaction as Overlay

A compaction event is an [`InternalEvent`][InternalEvent] variant — like
`ConfigDelta` — that modifies how *preceding* events are interpreted when
building the LLM request. It does not modify or delete any existing events.

```
InternalEvent::ConfigDelta  → "from here on, use this config"
InternalEvent::Compaction   → "when building the LLM view, apply these
                               policies to events in this range"
```

The original events remain in `events.json`. The projection layer in
`Thread::into_parts()` reads all compaction events, builds a projection plan,
and yields the appropriate view to the provider.

### User-Facing Behavior

#### The `compact` Command

```sh
jp conversation compact [ID] [OPTIONS]
```

Compacts the active conversation (or the specified one) by appending a
compaction event. The original events are untouched.

```sh
# Compact with workspace defaults
jp conversation compact

# Compact using a named profile
jp conversation compact --profile heavy

# Compact a specific range
jp conversation compact --from 5h --to 1h

# Compact everything except the last 3 turns
jp conversation compact --keep-last 3

# Preview what would change
jp conversation compact --dry-run
```

**Flags:**

| Flag               | Default               | Description                              |
|--------------------|-----------------------|------------------------------------------|
| `--profile <name>` | `default`             | Named compaction profile from config.    |
| `--from <bound>`   | start of conversation | Start of the compacted range             |
|                    |                       | (inclusive).                             |
| `--to <bound>`     | end of conversation   | End of the compacted range (inclusive).  |
| `--keep <N>`       | from config           | Shorthand for `--to` N turns ago.        |
| `--dry-run`        | `false`               | Preview mechanical effects without       |
|                    |                       | applying.                                |

Range bounds accept several formats:

| Value            | Example       | Meaning                                  |
|------------------|---------------|------------------------------------------|
| Positive integer | `--from 5`    | Absolute turn index (0-based).           |
| Negative integer | `--to -3`     | 3 turns before the last turn.            |
| Duration string  | `--from 5h`   | Time ago (resolved to a turn index).     |
| `last`           | `--from last` | Turn of the most recent compaction       |
|                  |               | event, or start if none.                 |

`--from` without a value defaults to `last`. All bounds are **resolved to
absolute turn indices at creation time** and stored as integers.

#### The `--compact` Flag on `query`

```sh
# Compact with default profile, then query
jp query --compact -- "Continue working on the feature"

# Compact with a named profile, then query
jp query --compact=heavy "Continue working on the feature"
```

Equivalent to `jp conversation compact` followed by `jp query`. `--compact`
alone uses the conversation's default profile; `--compact=<name>` uses the named
profile.

#### The `--compact` Flag on `fork`

```sh
# Fork and compact with default profile
jp conversation fork --compact

# Fork and compact with a named profile
jp conversation fork --compact=heavy
```

Forks the conversation and appends a compaction event to the fork. Uses the
forked conversation's resolved compaction config.

#### Viewing Compacted Conversations

```sh
# Print the full history (default)
jp conversation print

# Print the compacted view (what the LLM sees)
jp conversation print --compacted
```

### Compaction Event Model

#### The `Compaction` Type

A compaction event defines an explicit range and optional per-content-type
policies:

```rust
/// A compaction overlay stored in the event stream.
///
/// Defines how events within [from_turn, to_turn] should be projected
/// when building the LLM request. The original events are unmodified.
pub struct Compaction {
    pub timestamp: DateTime<Utc>,

    /// First turn in the compacted range (inclusive, 0-based).
    pub from_turn: usize,

    /// Last turn in the compacted range (inclusive, 0-based).
    pub to_turn: usize,

    /// When set, replaces ALL provider-visible events in the range
    /// with a pre-computed summary. Takes precedence over `reasoning`
    /// and `tool_calls`.
    pub summary: Option<SummaryPolicy>,

    /// Policy for ChatResponse::Reasoning events.
    /// Ignored when `summary` is set.
    pub reasoning: Option<ReasoningPolicy>,

    /// Policy for ToolCallRequest and ToolCallResponse pairs.
    /// Ignored when `summary` is set.
    pub tool_calls: Option<ToolCallPolicy>,
}
```

`None` means "this compaction has no opinion on this content type" — the
original events pass through, or an earlier compaction's policy applies.

#### Per-Content-Type Policies

Each content type has its own policy enum, carrying only what makes sense for
that type:

```rust
pub enum ReasoningPolicy {
    /// Omit all reasoning events from the projected view.
    Strip,
}

/// Replaces ALL provider-visible events in the range with a
/// pre-computed summary. Messages, reasoning, and tool calls are
/// all replaced by a single synthetic ChatRequest/ChatResponse pair.
pub struct SummaryPolicy {
    /// The summary text, generated at compaction-creation time.
    pub summary: String,
}

pub enum ToolCallPolicy {
    /// Replace request arguments and/or response content with compact
    /// summaries. Preserves tool name, call ID, and success/error status.
    ///
    /// Parses from strings for config ergonomics:
    /// - `"strip"` → Strip { request: true, response: true }
    /// - `"strip-responses"` → Strip { request: false, response: true }
    /// - `"strip-requests"` → Strip { request: true, response: false }
    ///
    /// Or inline table: `{ policy = "strip", request = true, response = true }`
    Strip {
        /// Replace arguments with a compact summary.
        request: bool,
        /// Replace response content with a status line.
        response: bool,
    },

    /// Remove all tool call pairs entirely.
    Omit,
}
```

#### Eagerness Principle

Transformations fall into two categories:

- **Eager (store the result).** Expensive or non-deterministic operations —
  LLM-generated summaries. The output is stored in the compaction event
  (`SummaryPolicy { summary }`) because regenerating it would be costly and
  potentially different each time.

- **Lazy (store the policy).** Cheap, deterministic operations — stripping
  reasoning, replacing tool responses with a status line, omitting events. The
  policy is stored (`ToolCallPolicy::StripResponses`), and the projection layer
  applies it at read time.

#### Integration with `InternalEvent`

The compaction event is a new variant of `InternalEvent`, alongside
`ConfigDelta` and `Event`:

```rust
pub enum InternalEvent {
    ConfigDelta(ConfigDelta),
    Event(Box<ConversationEvent>),
    Compaction(Compaction),
}
```

Like `ConfigDelta`, compaction events are stream metadata — they are not visible
to providers, not counted by `ConversationStream::len()`, and are preserved by
`retain()`.

### Projection Layer

The projection layer transforms the raw event stream into the view sent to the
LLM. It is applied in `Thread::into_parts()`, which already filters events via
`is_provider_visible()`.

#### Algorithm

1. **Collect** all `Compaction` events from the stream.
2. **For each** conversation event at turn T with content type C:
   a. Find all compaction events where `from_turn <= T <= to_turn` and the
      policy for C is `Some`.
   b. Of those, the one with the **latest timestamp** wins.
   c. Apply the winning policy: keep, omit, strip, or substitute.
3. **When `summary` is set**: omit ALL provider-visible events in the range
   (messages, reasoning, tool calls). Inject a synthetic `ChatRequest` /
   `ChatResponse::Message` pair at the `from_turn` position containing the
   pre-computed summary. When `summary` is set, the `reasoning` and `tool_calls`
   policies are ignored for events in the range.

This logic lives in a new `ConversationStream::projected_iter()` method (or
similar), called by `Thread::into_parts()` instead of the raw iterator.

#### Projection Example

A concrete example showing how the projection applies across event types:

```txt
Raw stream (turns 0-2, then turns 3+ uncompacted):

  Turn 0: TurnStart
  Turn 0: ChatRequest("set up the project")
  Turn 0: ChatResponse::Message("I'll create the project structure.")
  Turn 0: ToolCallRequest(id="1", fs_create_file, {path: "src/main.rs"})
  Turn 0: ToolCallResponse(id="1", ok, "<200 lines of code>")
  Turn 0: ChatResponse::Message("Created src/main.rs with a basic setup.")
  Turn 1: TurnStart
  Turn 1: ChatRequest("add error handling")
  Turn 1: ChatResponse::Reasoning("<500 tokens of thinking>")
  Turn 1: ToolCallRequest(id="2", fs_read_file, {path: "src/main.rs"})
  Turn 1: ToolCallResponse(id="2", ok, "<200 lines of code>")
  Turn 1: ToolCallRequest(id="3", fs_modify_file, {path: "src/main.rs"})
  Turn 1: ToolCallResponse(id="3", ok, "<300 lines of diff>")
  Turn 1: ChatResponse::Message("Added error handling to main.")
  Turn 2: TurnStart
  Turn 2: ChatRequest("now add logging")
  Turn 2: ChatResponse::Reasoning("<400 tokens of thinking>")
  Turn 2: ToolCallRequest(id="4", fs_modify_file, {path: "src/main.rs"})
  Turn 2: ToolCallResponse(id="4", ok, "<250 lines of diff>")
  Turn 2: ChatResponse::Message("Added tracing-based logging.")
```

With the `default` profile (`reasoning: Strip, tool_calls: Strip`):

```txt
Compaction event (after turn 2):
  from_turn: 0, to_turn: 2
  summary: None
  reasoning: Strip
  tool_calls: Strip { request: true, response: true }

Projected view:

  ChatRequest("set up the project")
  ChatResponse::Message("I'll create the project structure.")
  ToolCallRequest(id="1", fs_create_file, {[compacted]})
  ToolCallResponse(id="1", ok, "[compacted] fs_create_file: success")
  ChatResponse::Message("Created src/main.rs with a basic setup.")
  ChatRequest("add error handling")
  ToolCallRequest(id="2", fs_read_file, {path: "src/main.rs"})
  ToolCallResponse(id="2", ok, "[compacted] fs_read_file: success")
  ToolCallRequest(id="3", fs_modify_file, {[compacted]})
  ToolCallResponse(id="3", ok, "[compacted] fs_modify_file: success")
  ChatResponse::Message("Added error handling to main.")
  ChatRequest("now add logging")
  ToolCallRequest(id="4", fs_modify_file, {[compacted]})
  ToolCallResponse(id="4", ok, "[compacted] fs_modify_file: success")
  ChatResponse::Message("Added tracing-based logging.")
  ...turns 3+ uncompacted...
```

Reasoning is stripped, tool arguments and responses are compacted. Note that
`fs_read_file` at id="2" keeps its arguments (per-tool hint `request = "keep"`)
while `fs_create_file` and `fs_modify_file` have their arguments stripped
(per-tool hint `request = "strip"` because they carry large file content).
Messages and conversation structure are preserved.

With the `heavy` profile (`summary: Summarize`):

```txt
Compaction event (after turn 2):
  from_turn: 0, to_turn: 2
  summary: SummaryPolicy { summary: "Set up a Rust project at src/main.rs
    with error handling and tracing-based logging." }
  reasoning: None
  tool_calls: None

Projected view:

  ChatRequest("[Summary of previous conversation]")
  ChatResponse::Message("Set up a Rust project at src/main.rs
    with error handling and tracing-based logging.")
  ...turns 3+ uncompacted...
```

The two profiles show the distinction:

- **`default` (mechanical):** Conversation structure is preserved. Reasoning is
  stripped, tool responses are replaced with status lines. Messages and tool
  call requests remain — the model sees the full flow of what happened, minus
  the bulk.
- **`heavy` (summarization):** Everything in the range is replaced by a single
  summary. The summarizer reads ALL raw events (messages, reasoning, tool calls)
  to produce the summary, so tool usage and decisions are captured in the text.
  No orphaned events remain.

When `summary` is set, `reasoning` and `tool_calls` are ignored — the summary
replaces everything. They only apply when compacting without summarization.

#### Stacking Semantics

Multiple compaction events compose independently per content type. For each
event, per content type, the latest compaction whose range covers that event
wins.

Example:

```txt
Compaction A (turn 20): from=0, to=20, summary=SummaryPolicy("...")
Compaction B (turn 30): from=0, to=30, tool_calls=Strip { request: false, response: true }
```

| Turn | Event type   | A            | B     | Winner        |
|------|--------------|--------------|-------|---------------|
| 5    | Any          | Summarize    | —     | A: Summarize  |
| 5    | Tool calls   | Summarize    | Strip | A: Summarize* |
| 25   | Tool calls   | out of range | Strip | B: Strip      |
| 25   | Reasoning    | out of range | —     | Keep          |

\* `summary` takes precedence over per-type policies when both cover an event.

#### Summary Overlap Resolution

Summaries are holistic representations of a range — they cannot be split or
sliced. Partial overlaps between summary ranges would produce irreconcilable
conflicts (two summaries covering partially the same turns, potentially
contradicting each other).

**Rule: when creating a new compaction with `summary: Some(SummaryPolicy)`, if
any existing summary compaction partially overlaps with the new range, the new
range is auto-extended to fully subsume the existing one.**

Formally: given new range `[X, Y]` and existing summary range `[A, B]`, if the
ranges intersect but neither fully contains the other, extend to `[min(X, A),
max(Y, B)]`. Repeat until no partial overlaps remain. The summarizer then reads
raw events for the extended range.

This constraint applies only when `summary` is set. All other policies operate
per-event and compose naturally with partial overlaps.

#### Raw-Stream Invariant

**Summarization always reads the raw (non-compacted) event stream.** The
summarizer sees the original messages, not prior summaries. This prevents
compound information loss — summarizing a summary degrades quality at each step.

When compaction B's range overlaps with compaction A's range, B's summarizer
reads the original events for its full range, ignoring A's summary entirely. At
projection time, B's summary wins for the overlapping region (it has a later
timestamp), and it is a faithful summary of the originals.

This is already guaranteed by the additive design — the raw events are always in
`events.json` — but it is worth stating as an invariant: **no code path should
feed a projected view to a summarizer.**

### Strategies

A strategy is a function that analyzes a `ConversationStream` and produces a
`Compaction` event. Strategies do not mutate the stream.

#### Mechanical Strategies

These are pure transformations that don't require LLM calls.

##### `strip-reasoning`

Produces a compaction with `reasoning: Some(ReasoningPolicy::Strip)` for the
specified range.

**Impact:** Moderate token reduction for models that emit extended thinking.

##### `strip-tools`

Produces a compaction with `tool_calls: Some(ToolCallPolicy::Strip { .. })` for
the specified range. At projection time, tool response content is replaced with
a status line (`[compacted] {tool_name}: {success|error}`) and/or request
arguments are replaced with a compact summary. Which fields are stripped depends
on the profile and per-tool hints.

**Impact:** High. Tool responses and arguments (especially for file-writing
tools) dominate token count in coding conversations.

#### LLM-Assisted Strategies

##### `summarize`

Sends the raw events in the specified range to an LLM with instructions to
produce a concise summary. Produces a compaction with `summary:
Some(SummaryPolicy { summary })`. When set, this replaces all provider-visible
events in the range.

The summarization prompt instructs the model to preserve key decisions, file
paths, error resolutions, and the current state of the task. The model and
prompt are configurable per-profile (see [Configuration](#configuration)).

**Impact:** High. Replaces an arbitrary number of turns with a short summary.

### Configuration

Compaction is configured at the workspace and conversation level, following the
same defaults-plus-named-profiles pattern used by tool configuration.

```toml
[conversation.compaction]
# The profile to use when --profile is not specified.
default_profile = "default"

# Number of recent turns to preserve (used by profiles that don't
# override it). Shorthand for setting `to` to N turns ago.
keep_last = 3

# Default compaction profile. Applied by `--compact` with no arguments.
[conversation.compaction.profiles.default]
reasoning = "strip"
tool_calls = "strip"

# A heavier profile that includes summarization.
# When summary is set, it replaces all events in the range —
# reasoning and tool_calls policies are not needed.
[conversation.compaction.profiles.heavy.summary]
policy = "summarize"
model = "anthropic/claude-haiku"
# instructions = """
# Summarize this conversation for continuity. Preserve:
# - File paths and code structures discussed
# - Key decisions and their rationale
# - Current task state and next steps
# """

# A minimal profile for quick cleanup.
[conversation.compaction.profiles.light]
reasoning = "strip"
```

Profiles define which per-type policies to apply. The range (`from`, `to`,
`keep_last`) comes from the CLI flags or the top-level `keep_last` default. A
profile does not encode a range — ranges are an invocation-time concern.

Conversation-level overrides (via `--cfg`) can change any of these for a
specific conversation.

### Per-Tool Compaction Hints

Tools can declare how their calls should be compacted. This is a new optional
field in the tool configuration:

```toml
[conversation.tools.fs_read_file.compaction]
request = "keep" # "keep" | "strip"
response = "strip" # "keep" | "strip"
```

Per-tool hints override the profile's `Strip` policy for individual tools. A
tool with `response = "keep"` is exempted from response stripping even under a
policy that sets `response: true`.

Example defaults for the JP project:

| Tool             | `request` | `response` |
|------------------|-----------|------------|
| `fs_read_file`   | `keep`    | `strip`    |
| `fs_grep_files`  | `keep`    | `strip`    |
| `cargo_check`    | `keep`    | `strip`    |
| `cargo_test`     | `keep`    | `strip`    |
| `fs_create_file` | `strip`   | `keep`     |
| `fs_modify_file` | `strip`   | `strip`    |
| `git_commit`     | `strip`   | `keep`     |

These are workspace-level tool configurations, not built-in defaults. Each
workspace defines its own tools and compaction hints.

## Drawbacks

- **Summaries are lossy.** Even though the original events are preserved, the
  LLM only sees the compacted view. A poor summary can mislead the model worse
  than a long conversation. Mitigation: summaries are generated from raw events
  (never from prior summaries), and the summarization model and prompt are
  configurable.

- **Storage growth.** Compaction events add to the stream rather than reducing
  stored size. Summary text in `SummaryPolicy` can be non-trivial. In practice
  this is small compared to the tool responses they overlay, but it is additive
  rather than reductive.

- **Projection complexity.** The projection layer adds a code path between the
  raw stream and the LLM. Bugs in projection logic could cause the LLM to see
  inconsistent state. Mitigation: the projection is a pure function of the
  stream, fully testable without LLM calls.

- **Prompt cache invalidation.** Adding a compaction event changes the projected
  prefix, invalidating any cached conversation history. System prompt caching is
  unaffected (it is a separate prefix). This is acceptable for manual compaction
  but would be problematic for automatic compaction.

- **`--dry-run` cannot preview summaries.** For mechanical strategies, dry-run
  accurately shows the projected view. For summarization, dry-run can only
  report "will generate a summary for turns X-Y using model Z" — it cannot show
  the actual summary without spending tokens on an LLM call, and re-running
  without `--dry-run` would produce a different summary anyway.

## Alternatives

### Destructive compaction ([RFD 036])

The original design: strategies mutate the `ConversationStream` directly, with
fork-by-default as a safety net. Rejected because:

1. **Information loss.** Once events are deleted, they're gone. Fork mitigates
   but doesn't solve — you end up with two conversations, one intact and one
   damaged.
2. **No undo.** Reverting a compaction requires restoring from the fork.
3. **Fork proliferation.** Each compaction creates a new conversation,
   cluttering the conversation list.
4. **Conflated concerns.** Destructive compaction mixes "what to send to the
   LLM" (a view concern) with "what to store on disk" (a persistence concern).

### Automatic compaction on every turn

Compact transparently when approaching the context window limit. Rejected for
this RFD: compaction is lossy and should be an explicit user decision. Automatic
compaction has additional design constraints (caching interaction, interval
control, trigger conditions) that warrant a separate proposal.

### Single monolithic compact operation

One "compact" that does everything. Rejected: different conversations need
different compaction. A coding conversation benefits from tool response
stripping; a discussion benefits from summarization. Named profiles with
per-type policies let users tailor the operation.

## Non-Goals

- **Automatic compaction.** This RFD covers explicit, user-initiated compaction.
  Automatic compaction (triggered by context window proximity or turn count
  thresholds) has different design constraints — caching interaction, trigger
  intervals, rolling window semantics — and is deferred to a follow-up RFD. The
  config namespace (`conversation.compaction.auto`) is reserved.

- **Provider-delegated compaction.** Some providers offer server-side compaction
  ([Anthropic][anthropic-compaction] returns readable summaries,
  [OpenAI][openai-compaction] returns opaque encrypted blobs). In practice,
  readable provider summaries offer no advantage over JP's own `SummaryPolicy`
  using the same model, and opaque formats cannot be integrated into JP's event
  model. Provider delegation may become interesting if providers offer
  compaction capabilities that can't be replicated client-side, but that's not
  the case today.

- **Custom external strategies.** An extension point where an external binary
  receives the raw events and range, and returns replacement events that JP
  stores in the compaction event. This is analogous to how tools work today
  (external process, structured I/O) and would enable domain-specific compaction
  logic. The compaction event model supports this (replacement events are just
  the policy payloads), but the protocol and CLI integration are deferred.

- **Tool subsumption protocol.** [RFD 036] proposed an `Action::Subsumes` tool
  protocol extension where tools could declare that one call subsumes another
  (e.g., `read_file(1,10)` subsumes `read_file(2,5)`). This is a refinement that
  can be added later without changing the compaction event model.

- **Interactive tangent classification.** [RFD 036] proposed a
  `classify-tangents` strategy that uses an LLM to identify off-topic turns.
  Interesting but orthogonal to the core compaction model.

- **Tool call deduplication.** Identifying and removing duplicate tool calls
  (same name, same arguments) across turns. While potentially useful, it adds
  complexity to the compaction model (per-call selective policies) for marginal
  benefit. Can be added as a `ToolCallPolicy::Selective` variant later if
  needed.

- **Conversation merging.** Combining two conversations into one.

## Risks and Open Questions

- **Summarization prompt design.** The summary needs to preserve the right
  context — key decisions, file paths, error resolutions, task state. What
  should the prompt look like? This needs experimentation during implementation.
  We should take inspiration from Anthropic's default summarization prompt.

- **Turn boundary correctness.** Range resolution must handle edge cases:
  conversations with only 1 turn, turns with no tool calls, interrupted turns.
  The existing `fork --last` implementation is a reference.

- **Config delta preservation.** `ConversationStream` interleaves `ConfigDelta`
  events with conversation events. The projection layer must preserve config
  deltas correctly — compacting a range should not affect config state for
  events outside that range.

- **Summary injection and provider expectations.** The synthetic
  `ChatRequest`/`ChatResponse` pair injected for summaries must maintain the
  user/assistant alternation that providers expect. Needs testing across
  Anthropic, OpenAI, Google, and local providers.

## Implementation Plan

### Phase 1: Compaction Event Model

1. Add `InternalEvent::Compaction(Compaction)` to `jp_conversation`.
2. Define the `Compaction` struct, per-type policy enums, and serialization.
3. Update `ConversationStream` to handle the new variant: `is_empty()`, `len()`,
   `retain()`, `sanitize()` should treat compaction events like config deltas
   (preserved, not counted).
4. Add unit tests for serialization roundtrip and stream invariants.

Can be merged independently. No behavioral changes.

### Phase 2: Projection Layer

1. Add `ConversationStream::projected_iter()` that applies compaction overlays
   to yield the projected view.
2. Implement the stacking semantics (latest-wins per content type).
3. Implement summary injection (synthetic `ChatRequest`/`ChatResponse` pair).
4. Wire `Thread::into_parts()` to use `projected_iter()`.
5. Add unit tests for each policy type, stacking, and summary overlap
   auto-extension.

Depends on Phase 1. After this phase, compaction events in the stream will
affect what the LLM sees.

### Phase 3: Mechanical Strategies and CLI

1. Implement strategy functions: `strip_reasoning`, `strip_tools`. Each produces
   a `Compaction` event.
2. Implement range bound resolution (negative integers, duration strings, `last`
   → absolute turn index).
3. Add the `jp conversation compact` CLI command with `--profile`, `--from`,
   `--to`, `--keep-last`, `--dry-run`.
4. Add `--compact[=profile]` to `jp conversation fork`.
5. Add `--compacted` to `jp conversation print`.
6. Add integration tests.

Depends on Phase 2.

### Phase 4: Configuration

1. Add `conversation.compaction` config section with `default_profile`,
   `keep_last`.
2. Add `conversation.compaction.profiles` support (named policy sets).
3. Add per-tool `compaction` hints to `ToolConfig`.
4. Wire profiles into the CLI (`--profile` flag, `--compact` defaults).
5. Add config tests.

Depends on Phase 3. Can be partially parallelized with Phase 3 (config types can
be defined before the CLI is wired up).

### Phase 5: LLM-Assisted Summarization

1. Implement the `summarize` strategy: read raw events, call the configured
   model, produce `SummaryPolicy { summary }`.
2. Implement the summary overlap auto-extension logic.
3. Add `--compact[=profile]` to `jp query`.
4. Add integration tests (with mock LLM).

Depends on Phase 2. Can proceed in parallel with Phases 3 and 4.

## References

- [RFD 011] — System Notification Queue (compaction interaction)
- [RFD 034] — Inquiry-Specific Assistant Configuration (defers compaction)
- [RFD 036] — Conversation Compaction (superseded by this RFD)
- [Issue #57] — Make conversation management more powerful
- [Multi-turn degradation paper][paper] — cited in Issue #57

[RFD 011]: 011-system-notification-queue.md
[RFD 034]: 034-inquiry-specific-assistant-configuration.md
[RFD 036]: 036-conversation-compaction.md
[InternalEvent]: https://github.com/dcdpr/jp/blob/main/crates/jp_conversation/src/stream.rs
[Issue #57]: https://github.com/dcdpr/jp/issues/57
[anthropic-compaction]: https://docs.anthropic.com/en/docs/build-with-claude/compaction.md
[openai-compaction]: https://developers.openai.com/api/docs/guides/compaction.md
[paper]: https://arxiv.org/abs/2505.06120
