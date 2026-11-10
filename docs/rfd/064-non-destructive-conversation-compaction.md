# RFD 064: Non-Destructive Conversation Compaction

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-17
- **Supersedes**: [RFD 036]
- **Summary**: Compacts conversations non-destructively by appending overlay
  events that project a reduced view to the LLM without mutating stored history.

## Summary

This RFD introduces non-destructive conversation compaction.
Instead of mutating stored history, compaction appends overlay events that
project a reduced view when building LLM requests.
Policies are defined per content type (summary, reasoning, and tool calls),
compose across multiple compaction events, and can be configured at the
workspace and conversation levels.

## Motivation

Long-running conversations degrade LLM performance.
When a model takes a wrong turn early in a conversation, it rarely recovers (see
[Issue #57]).
Even when the conversation stays on track, growing context windows cause:

1. **Higher cost.** Every cached and uncached input token is billed.
   Tool call responses (file contents, grep results, and test output) dominate
   the token count in coding conversations.
2. **Slower responses.** More input tokens increase time-to-first-token.
3. **Lower quality.** Models lose focus in long contexts, and obsolete tool
   results and abandoned tangents mislead them.
4. **Context window overflow.** Eventually the conversation exceeds the model's
   context window and fails.

Users currently work around this by forking the last turn (`jp conversation fork
--last 1`), which discards useful context along with the noise.

JP needs a way to *selectively* reduce conversation size while preserving the
context that matters.
Multiple existing RFDs defer to this one:

- [RFD 011] (System Notification Queue): "If JP ever implements conversation
  compaction..."
- [RFD 034] (Inquiry Config): "smarter compaction (summarization, middle-out
  trimming) is orthogonal"

[RFD 036] proposed compaction as a destructive transformation of the
`ConversationStream`, using strategies that mutate or replace events and
fork-by-default as a safety net.
This RFD supersedes that design with a non-destructive approach: compaction
events are appended to the stream and define a *projection* of the original
events, preserving the full history while sending a reduced view to the LLM.

## Design

### Core Concept: Compaction as Overlay

A compaction event is an [`InternalEvent`][InternalEvent] variant, like
`ConfigDelta`, that changes how *preceding* events are interpreted when building
an LLM request without modifying or deleting them.

```
InternalEvent::ConfigDelta  → "from here on, use this config"
InternalEvent::Compaction   → "when building the LLM view, apply these
                               policies to events in this range"
```

The original events stay in `events.json`.
The projection layer in `Thread::into_parts()` reads all compaction events,
builds a projection plan, and yields the reduced view to the provider.

### User-Facing Behavior

#### The `compact` Command

```sh
jp conversation compact [ID] [OPTIONS]
```

Compacts the active conversation (or `ID`, if given) by appending one or more
compaction events from the configured rules, leaving the original events
untouched.

```sh
# Compact with workspace defaults
jp conversation compact

# Compact with overrides from a config file
jp -c compaction/heavy conversation compact

# Override range via flags
jp conversation compact --keep-last 5

# Compact a specific range
jp conversation compact --from 5h --to 1h

# Strip only reasoning
jp conversation compact --reasoning

# Preview what would change
jp conversation compact --dry-run

# Remove all compaction events (undo)
jp conversation compact --reset
```

**Flags:**

| Flag                | Default               | Description                             |
| ------------------- | --------------------- | --------------------------------------- |
| `--keep-first <N>`  | from config           | Preserve the first N turns.             |
| `--keep-last <N>`   | from config           | Preserve the last N turns.              |
| `--from <bound>`    | start of conversation | Start of the compacted range            |
|                     |                       | (inclusive). Overrides `--keep-first`.  |
| `--to <bound>`      | end of conversation   | End of the compacted range (inclusive). |
|                     |                       | Overrides `--keep-last`.                |
| `--reasoning`       | from config           | Strip reasoning (thinking) blocks.      |
| `--tools`           | from config           | Strip tool call arguments/responses.    |
| `--summary [TEXT]`  | from config           | Replace the range with a generated      |
|                     |                       | summary, or with `TEXT` verbatim when   |
|                     |                       | given.                                  |
| `--summary-context` | none                  | Extra guidance for the summarizer.      |
|                     |                       | Only affects rules that generate a      |
|                     |                       | summary.                                |
| `--dry-run`         | `false`               | Preview effects without applying.       |
| `--reset`           | `false`               | Remove all compaction events from the   |
|                     |                       | stream.                                 |

> [!TIP]
> `--summary` replaced `--summarize`, whose optional value passed extra guidance
> to the summarizer instead of supplying the summary text.
> Passing `--summarize TEXT` fails rather than silently storing `TEXT` as the
> summary; pass summarizer guidance with `--summary-context` instead.

Range bounds accept several formats:

| Value            | Example       | Meaning                              |
| ---------------- | ------------- | ------------------------------------ |
| Positive integer | `--from 5`    | Absolute turn index (0-based).       |
| Negative integer | `--to -3`     | 3 turns before the last turn.        |
| Duration string  | `--from 5h`   | Time ago (resolved to a turn index). |
| `last`           | `--from last` | Turn of the most recent compaction   |
|                  |               | event, or start if none.             |

`--from` requires a value.
All bounds are **resolved to absolute turn indices at creation time** and stored
as integers.

> [!NOTE]
> CLI turn positions are **1-based** (`--from 1` is the first turn and `--to -1`
> is the last), while stored indices remain 0-based.
> The `5` and `-3` examples above and the DSL bounds below reflect the original
> 0-based convention; see [Indexing and Counting Conventions] for the 1-based
> rule.
> Under 1-based indexing, `-N` selects the Nth turn from the end rather than
> preserving N turns: `s:..-3` compacts through the third turn from the end
> (preserving the last two turns), while `s:..-4` and `keep_last = 3` both
> preserve the last three turns.
>
> The `last` keyword was renamed to `last-compaction` and is valid only on
> `--from`.

`--reset` undoes compaction by removing all `InternalEvent::Compaction` events
from the stream so the LLM sees the uncompacted history.

> [!NOTE]
> `--reset` accepts an optional 1-based index (`--reset=2`) to remove a single
> compaction event, numbered as listed by `jp conversation show`.
> Passing an index requires `=`, so a bare `--reset` followed by a positional
> argument still parses that argument as the conversation ID.

#### The `--compact` Flag (DSL)

`query`, `fork`, and `compact` accept `--compact` (short flag `-k`) in two
forms:

- **Bare `--compact`**: apply the compaction rules from the resolved
  configuration.
- **`--compact=SPEC`**: apply an inline DSL rule.
  Each flag adds one compaction event.

The two forms compose: passing bare `--compact` alongside `--compact=SPEC`
applies the configured rules followed by each inline rule.
Passing `--compact=SPEC` without bare `--compact` applies only the inline rules
and ignores the configured rules.

```sh
# Apply config rules, then query
jp query --compact -- "Continue working on the feature"

# Apply config rules via short flag
jp query -k -- "Continue"

# Inline DSL: summarize all but last 3, then query
jp query -k s:..-3 -- "Continue"

# Two inline rules on fork
jp conversation fork -k r:..-20 -k s:..-3

# Mix config rules + inline rule
jp query --compact -k s:..-1 -- "Continue"
```

##### DSL Grammar

```
SPEC     = POLICIES [":" RANGE]
POLICIES = POLICY ["+" POLICY]*
POLICY   = "r" | "reasoning"
         | "t" | "tools"
         | "s" | "summary"       # "summarize" is accepted as an alias
RANGE    = [BOUND] ".." [BOUND]    # explicit range (at least "..")
         | BOUND                    # single-bound shorthand
BOUND    = INTEGER                 # >= 0: absolute turn index
         | "-" INTEGER             # <  0: offset from the end
```

The range specifies **which turns the policy applies to** (`from_turn` through
`to_turn` in the `Compaction` event, both **inclusive**); turns outside the
range are unaffected.
Bounds follow Python slice conventions: a non-negative integer is a 0-based turn
index, and a negative integer is an offset from the end (`-1` is the last turn).
Either bound can use either form.
Omitting the start bound defaults to the first turn; omitting the end bound
defaults to the last turn.

**Range semantics:**

Full `FROM..TO` form:

| Syntax    | Meaning                                            |
| --------- | -------------------------------------------------- |
| `..`      | All turns (start to end).                          |
| `5..`     | Turn 5 through the end (keeps the first 5).        |
| `..-3`    | Start through 3-from-end (keeps the last 3).       |
| `5..-3`   | Turn 5 through 3-from-end (keeps first 5, last 3). |
| `5..10`   | Absolute turns 5 through 10.                       |
| `..10`    | Start through absolute turn 10.                    |
| `-10..-3` | 10-from-end through 3-from-end.                    |

Single-bound shorthands:

| Syntax | Expands to | Meaning                       |
| ------ | ---------- | ----------------------------- |
| `-3`   | `..-3`     | Keep last 3 uncompacted.      |
| `5`    | `5..`      | Turn 5 onward (keep first 5). |

**Examples:**

| DSL spec   | Meaning                                     |
| ---------- | ------------------------------------------- |
| `s`        | Summarize, range from config defaults.      |
| `r+t`      | Strip reasoning + tools, range from config. |
| `s:..-3`   | Summarize all but last 3 turns.             |
| `r+t:..-3` | Strip reasoning + tools, keep last 3.       |
| `s:..`     | Summarize all events.                       |
| `r:5..`    | Strip reasoning from turn 5 onward.         |
| `s:5..-3`  | Summarize turns 5 through 3-from-end.       |
| `s:5..10`  | Summarize absolute turns 5 through 10.      |
| `s:-3`     | Summarize all but last 3 (shorthand).       |
| `r:-20`    | Strip reasoning, keep last 20 (shorthand).  |

Omitting the range falls back to the configured `keep_first` and `keep_last`
defaults.
Policies in a DSL spec are self-contained and never inherit policies from the
configuration.

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
/// Defines how events within [from_turn, to_turn] should be projected when
/// building the LLM request.
/// The original events are unmodified.
pub struct Compaction {
    pub timestamp: DateTime<Utc>,

    /// First turn in the compacted range (inclusive, 0-based).
    pub from_turn: usize,

    /// Last turn in the compacted range (inclusive, 0-based).
    pub to_turn: usize,

    /// When set, replaces ALL provider-visible events in the range with a
    /// pre-computed summary.
    /// Takes precedence over `reasoning` and `tool_calls`.
    pub summary: Option<SummaryPolicy>,

    /// Policy for ChatResponse::Reasoning events.
    /// Ignored when `summary` is set.
    pub reasoning: Option<ReasoningPolicy>,

    /// Policy for ToolCallRequest and ToolCallResponse pairs.
    /// Ignored when `summary` is set.
    pub tool_calls: Option<ToolCallPolicy>,
}
```

A `None` policy leaves that content type unchanged, allowing the original events
or an earlier compaction's policy to pass through.

#### Per-Content-Type Policies

Each content type has its own policy type:

```rust
pub enum ReasoningPolicy {
    /// Omit all reasoning events from the projected view.
    Strip,
}

/// Replaces ALL provider-visible events in the range with a pre-computed
/// summary.
/// Messages, reasoning, and tool calls are all replaced by a single synthetic
/// ChatRequest/ChatResponse pair.
pub struct SummaryPolicy {
    /// The summary text, generated at compaction-creation time.
    pub summary: String,
}

pub enum ToolCallPolicy {
    /// Replace request arguments and/or response content with compact
    /// summaries.
    /// Preserves tool name, call ID, and success/error status.
    ///
    /// Parses from strings for config ergonomics:
    ///
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

Compaction policies are either eager or lazy:

- **Eager (store the result).** Expensive or non-deterministic transformations,
  such as LLM-generated summaries.
  The compaction event stores the generated output (`SummaryPolicy { summary }`)
  so projection does not pay to regenerate a potentially different summary on
  every read.

- **Lazy (store the policy).** Cheap, deterministic transformations, such as
  stripping reasoning, replacing tool responses with a status line, or omitting
  events.
  The compaction event stores the policy (`ToolCallPolicy::StripResponses`), and
  the projection layer applies it at read time.

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

Like `ConfigDelta`, compaction events are stream metadata: they are invisible to
providers, excluded from `ConversationStream::len()`, and preserved by
`retain()`.

### Projection Layer

The projection layer transforms the raw event stream into the view sent to the
LLM.
It runs in `Thread::into_parts()`, which already filters events via
`is_provider_visible()`.

#### Algorithm

1. **Collect** all `Compaction` events from the stream.
2. **Resolve per-event policies.** For each conversation event at turn `T` with
   content type `C`:
   1. Find all compaction events where `from_turn <= T <= to_turn` that set a
      policy for `C`.
   2. Select the matching compaction event with the **latest timestamp**.
   3. Apply the winning policy (keep, omit, strip, or substitute).
3. **Apply summaries.** When a compaction sets `summary`, omit all
   provider-visible events in `[from_turn, to_turn]` (messages, reasoning, and
   tool calls), ignoring any `reasoning` or `tool_calls` policies in that range,
   and inject a synthetic `ChatRequest` / `ChatResponse::Message` pair at
   `from_turn` containing the pre-computed summary.

This logic lives in `ConversationStream::projected_iter()`, which
`Thread::into_parts()` calls in place of the raw iterator.

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

With default config (`reasoning: strip, tool_calls: strip`):

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
  ToolCallRequest(id="2", fs_read_file, {[compacted]})
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

Reasoning is stripped, and every tool call's arguments and responses are
compacted uniformly.
Messages and conversation structure are preserved.
Selective, per-tool exemptions (e.g. keeping `fs_read_file` arguments while
stripping `fs_create_file` content) are deferred to a future RFD.

With a summarization config (`-c compaction/heavy`):

```txt
Compaction event (after turn 2):
  from_turn: 0, to_turn: 2
  summary: SummaryPolicy { summary: "Set up a Rust project at src/main.rs
    with error handling and tracing-based logging." }
  reasoning: None
  tool_calls: None

Projected view:

  ChatRequest("[Summary of the earlier part of this conversation.
    The messages after it are still in your context.]")
  ChatResponse::Message("Set up a Rust project at src/main.rs
    with error handling and tracing-based logging.")
  ...turns 3+ uncompacted...
```

These two configurations show the distinction:

- **Mechanical (default):** Conversation structure is preserved.
  Reasoning is stripped, and tool responses are replaced with status lines.
  Messages and tool call requests remain, so the model sees the full flow of
  what happened without the bulk.
- **Summarization (heavy):** Everything in the range is replaced by a single
  summary.
  The summarizer reads the range's stored events (messages, reasoning, and tool
  calls) to produce the summary, capturing tool usage and decisions in the text
  without leaving orphaned events.

When `summary` is set on a rule, the summary replaces the entire range at
projection time, so `reasoning` and `tool_calls` on that rule have no further
effect on the projected view.
However, those mechanical policies still filter the input sent to the
summarizer: a rule with both `reasoning = "strip"` and `summary` strips
reasoning blocks before summarizing the range, whereas a rule with `summary`
alone summarizes the unfiltered events in the range.

#### Stacking Semantics

Multiple compaction events compose independently per content type.
For each event and content type, the latest compaction whose range covers that
event wins.

Example:

```txt
Compaction A (turn 20): from=0, to=20, summary=SummaryPolicy("...")
Compaction B (turn 30): from=0, to=30, tool_calls=Strip { request: false, response: true }
```

| Turn | Event type | A            | B     | Winner        |
| ---- | ---------- | ------------ | ----- | ------------- |
| 5    | Any        | Summarize    | none  | A: Summarize  |
| 5    | Tool calls | Summarize    | Strip | A: Summarize* |
| 25   | Tool calls | out of range | Strip | B: Strip      |
| 25   | Reasoning  | out of range | none  | Keep          |

\* `summary` takes precedence over per-type policies when both cover an event.

Each rule in a multi-rule configuration or DSL invocation produces a separate
compaction event, and the projection layer composes them at read time.
Rule ordering does not affect correctness: the projection resolves conflicts by
timestamp, and summaries always read the raw (uncompacted) stream.

#### Summary Overlap Resolution

Because a summary represents a range as a whole and cannot be split or sliced,
partially overlapping summary ranges would produce conflicting summaries for the
shared turns.

**Rule: when creating a compaction with `summary: Some(SummaryPolicy)`, if any
existing summary compaction partially overlaps the new range, the new range
automatically extends to subsume it.**

Given a new range `[X, Y]` and an existing summary range `[A, B]` that intersect
without one containing the other, the range extends to `[min(X, A), max(Y, B)]`.
This repeats until no partial overlaps remain, and the summarizer then reads the
stored events for the extended range.

This rule applies only when `summary` is set; mechanical policies operate per
event and compose across partial overlaps without extension.

#### Raw-Stream Invariant

**No prior compaction reaches a summarizer.** A summarizer always reads the
stored events in `events.json`, never the output of an earlier compaction,
preventing the compound information loss of summarizing a summary.

When compaction B's range overlaps with an earlier compaction A, B's summarizer
reads the raw stored events across its full range, ignoring both A's summary and
A's mechanical policies.
At projection time, B's summary takes precedence over A in the overlapping range
because it has a later timestamp.

The only filtering applied before summarization comes from mechanical policies
declared on the *same* compaction rule (see [Strategies](#strategies)), which
narrow the summarizer's input without compounding across separate compactions.

### Strategies

A strategy is a function that analyzes a `ConversationStream` and produces a
`Compaction` event without mutating the stream.

#### Mechanical Strategies

Pure transformations that do not require LLM calls.

##### `strip-reasoning`

Produces a compaction with `reasoning: Some(ReasoningPolicy::Strip)` for the
specified range.

**Impact:** Moderate token reduction for models that emit extended thinking.

##### `strip-tools`

Produces a compaction with `tool_calls: Some(ToolCallPolicy::Strip { .. })` for
the specified range.
Depending on the rule configuration, projection replaces tool response content
with a status line (`[compacted] {tool_name}: {success|error}`), replaces
request arguments with a compact summary, or both.

**Impact:** High.
Tool responses and arguments (especially for file-writing tools) dominate token
count in coding conversations.

#### LLM-Assisted Strategies

##### `summarize`

Filters the range's stored events through any `reasoning` and `tool_calls`
policies on the same rule, sends them to an LLM for summarization, and produces
a compaction with `summary: Some(SummaryPolicy { summary })` that replaces all
provider-visible events in the range.

The summarization prompt instructs the model to preserve key decisions, file
paths, error resolutions, and the current state of the task.
The model and prompt are configurable (see [Configuration](#configuration)).

**Impact:** High.
Replaces an arbitrary number of turns with a short summary.

### Configuration

Compaction is configured at the workspace and conversation levels under
`conversation.compaction`.
Configuration defines an ordered list of compaction **rules**, each producing
one `Compaction` event when applied.
Workspace and conversation variants use JP's standard config layering (`-c` and
`config.d/` directories) rather than a custom profile mechanism.

```toml
[conversation.compaction]
# Reserved for future features (e.g. auto-compaction).

# Rules are applied in order. Each rule produces one compaction event.
[[conversation.compaction.rules]]
keep_first = 1
keep_last = 3
reasoning = "strip"
tool_calls = "strip"
```

To define alternative compaction configurations, create config files in the
workspace's `config.d/` directory and load them with `-c`:

```toml
# .jp/config.d/compaction/heavy.toml
#
# Usage: jp -c compaction/heavy conversation compact
#        jp -c compaction/heavy query --compact -- "Continue"

[[conversation.compaction.rules]]
keep_last = 20
reasoning = "strip"

[[conversation.compaction.rules]]
keep_first = 1
keep_last = 3
tool_calls = "strip"

[conversation.compaction.rules.summary.model]
id = "anthropic/claude-haiku"
```

```toml
# .jp/config.d/compaction/light.toml
#
# Usage: jp -c compaction/light conversation compact

[[conversation.compaction.rules]]
keep_last = 5
reasoning = "strip"
```

Multiple `-c` files compose via `MergedVec` append semantics:

```sh
# Appends both rule sets: strip reasoning + summarize middle
jp -c compaction/strip-reasoning -c compaction/summarize-middle conversation compact
```

#### Rules Array and Merging

The `rules` field is a `MergedVec<CompactionRuleConfig>` with `append` as the
default merge strategy.
When multiple config sources define rules, they are concatenated in load order.

The built-in default rule (strip reasoning and tool calls, preserving the last 3
turns) sets `discard_when_merged: true`.
`jp conversation compact` applies this default when no rules or DSL specs are
given, and drops it as soon as any user-defined rule is present.

#### Rule Fields

Each rule in the array defines a single compaction operation:

| Field        | Type              | Default | Description                     |
| ------------ | ----------------- | ------- | ------------------------------- |
| `keep_first` | turns or duration | `1`     | Turns to preserve at the start. |
| `keep_last`  | turns or duration | `3`     | Turns to preserve at the end.   |
| `reasoning`  | `"strip"`         | none    | Strip reasoning blocks.         |
| `tool_calls` | mode string       | none    | Strip or omit tool calls.       |
| `summary`    | table             | none    | Generate an LLM summary.        |

`keep_first` and `keep_last` accept a turn count, a duration string (e.g.
`"5h"`), or a from-end position (e.g. `"-4"`); `keep_first` also accepts
`"last-compaction"`.
See [Indexing and Counting Conventions] for the supported bound syntax on each
field.

`tool_calls` accepts `"strip"` (both requests and responses),
`"strip-responses"`, `"strip-requests"`, or `"omit"`.

`summary` is a nested table with an optional `instructions` prompt and a `model`
table mirroring `assistant.model` (configured via `summary.model.id`):

```toml
[conversation.compaction.rules.summary]
instructions = "..." # optional, custom summarization prompt

# optional, defaults to the main assistant model
[conversation.compaction.rules.summary.model]
id = "anthropic/claude-haiku"
```

When `summary` is set, the summary replaces all events in the range at
projection time; `reasoning` and `tool_calls` on the same rule only filter the
input sent to the summarizer.

### Per-Tool Compaction Hints

Compaction applies `tool_calls` policies uniformly across all tools.
Per-tool hints that exempt individual tools' request arguments or response
content from stripping are deferred to a future RFD.

## Drawbacks

- **Summaries are lossy.** Although the original events remain on disk, the LLM
  sees only the compacted view, and a poor summary can mislead the model more
  than a long conversation.
  Mitigation: summaries are always generated from the stored events rather than
  prior projections, and the summarization model and prompt are configurable.

- **Storage growth.** Appending compaction events increases `events.json` size
  instead of shrinking it, especially when `SummaryPolicy` stores summary text,
  though the added bytes are small compared to the tool responses they overlay.

- **Projection complexity.** The projection layer introduces a transformation
  between the raw stream and the LLM where bugs could expose inconsistent state.
  Mitigation: projection is a pure function of the stream and can be unit-tested
  without LLM calls.

- **Prompt cache invalidation.** Adding a compaction event changes the projected
  conversation prefix and invalidates cached conversation history (system prompt
  caching uses a separate prefix and is unaffected).
  This cost is acceptable for manual compaction, though automatic compaction
  will need to account for it.

- **`--dry-run` cannot preview summaries.** Dry-run shows the exact projected
  view for mechanical strategies, but for summarization it can only report that
  turns `X..Y` will be summarized with model `Z`.
  Generating a preview summary would spend tokens on an LLM call whose output
  would not match a subsequent real run anyway.

## Alternatives

### Destructive compaction ([RFD 036])

The original design: strategies mutate the `ConversationStream` directly, with
fork-by-default as a safety net.
Rejected because:

1. **Information loss.** Deleted events cannot be recovered in place; forking
   only leaves behind two conversations, one intact and one truncated.
2. **No undo.** Reverting a compaction requires restoring from the fork.
3. **Fork proliferation.** Each compaction creates a new conversation,
   cluttering the conversation list.
4. **Conflated concerns.** Destructive compaction mixes "what to send to the
   LLM" (a view concern) with "what to store on disk" (a persistence concern).

### Named compaction profiles

A `profiles` map keyed by name (e.g. `default`, `heavy`, `light`) under
`conversation.compaction`, selected via a `--profile` flag.
Rejected because JP's config pipeline already supports named variants via
`config.d/` files and `-c`; a dedicated profile map would duplicate config
layering.

### Automatic compaction on every turn

Compact transparently when approaching the context window limit.
Rejected for this RFD: compaction is lossy and should be an explicit user
decision.
Automatic compaction has additional design constraints (caching interaction,
interval control, trigger conditions) that warrant a separate proposal.

### Single monolithic compact operation

A single compaction mode that applies fixed transformations.
Rejected because different conversations need different strategies: coding
sessions benefit from stripping tool responses, while design discussions benefit
from summarization.
Composable rules with per-type policies handle both.

## Non-Goals

- **Automatic compaction.** This RFD covers explicit, user-initiated compaction.
  Automatic compaction triggered by context window proximity or turn-count
  thresholds involves additional constraints (prompt cache interaction, trigger
  intervals, and rolling windows) and is deferred to a follow-up RFD.
  The `conversation.compaction.auto` config namespace is reserved.

- **Provider-delegated compaction.** Some providers offer server-side compaction
  ([Anthropic][anthropic-compaction] returns readable summaries;
  [OpenAI][openai-compaction] returns opaque encrypted blobs).
  Readable provider summaries offer no advantage over `SummaryPolicy` using the
  same model, and opaque blobs cannot integrate into JP's event stream.

- **Custom external strategies.** Running an external binary that receives raw
  events and a range over structured I/O and returns replacement events for the
  compaction payload.
  The `Compaction` event model accommodates external payloads, but the protocol
  and CLI integration are deferred.

- **Tool subsumption protocol.** [RFD 036] proposed an `Action::Subsumes` tool
  protocol extension allowing a tool to declare that one call subsumes another
  (for example, `read_file(1,10)` subsuming `read_file(2,5)`).
  This refinement can be added later without changing the compaction event
  model.

- **Interactive tangent classification.** [RFD 036] proposed a
  `classify-tangents` strategy that uses an LLM to identify off-topic turns,
  which is orthogonal to the core overlay model.

- **Tool call deduplication.** Removing duplicate tool calls with identical
  names and arguments across turns requires per-call selective policies for
  marginal benefit.
  It can be added later as a `ToolCallPolicy::Selective` variant if needed.

- **Conversation merging.** Combining two conversations into one.

- **Interactive stream editing.** An `$EDITOR` workflow for manually removing or
  reordering stored events (similar to `git rebase -i`).
  Mutating stored events is distinct from compaction's non-destructive overlay
  model and warrants a separate RFD.

## Risks and Open Questions

- **Summarization prompt design.** Summaries must preserve key decisions, file
  paths, error resolutions, and task state.
  The prompt design needs experimentation during implementation, drawing on
  Anthropic's default summarization prompt.

- **Turn boundary correctness.** Range resolution must handle single-turn
  conversations, turns without tool calls, and interrupted turns, following the
  existing `fork --last` implementation.

- **Config delta preservation.** `ConversationStream` interleaves `ConfigDelta`
  events with conversation events.
  The projection layer must preserve config deltas so compacting a range never
  alters the active configuration for events outside that range.

- **Summary injection and provider expectations.** The synthetic
  `ChatRequest`/`ChatResponse` pair injected for summaries must preserve the
  user/assistant role alternation required by Anthropic, OpenAI, Google, and
  local providers.

- **Migration of `Event::Patch` to the overlay model.** `Event::Patch` (used to
  recover from stale thinking-block signatures on Anthropic and Google
  providers) mutates historical events in the stream in place, deviating from
  the append-only model.
  Once Phase 2's projection layer lands, `Event::Patch` should append a
  metadata-patch overlay event applied at projection time, and `PatchAction`
  should not expand beyond `RemoveMetadata` until that migration is complete.

## Implementation Plan

### Phase 1: Compaction Event Model

1. Add `InternalEvent::Compaction(Compaction)` to `jp_conversation`.
2. Define the `Compaction` struct, per-type policy enums, and serialization.
3. Update `ConversationStream` to handle the new variant: `is_empty()`, `len()`,
   `retain()`, `sanitize()` should treat compaction events like config deltas
   (preserved, not counted).
4. Add unit tests for serialization roundtrip and stream invariants.

Can be merged independently.
No behavioral changes.

### Phase 2: Projection Layer

1. Add `ConversationStream::apply_projection()` that applies compaction overlays
   to transform the event list.
2. Implement the stacking semantics (latest-wins per content type).
3. Implement summary injection (synthetic `ChatRequest`/`ChatResponse` pair).
4. Wire `Thread::into_parts()` to call `apply_projection()`.
5. Add unit tests for each policy type, stacking, and summary overlap
   auto-extension.

Depends on Phase 1.
After this phase, compaction events in the stream affect what the LLM sees.

### Phase 3: Mechanical Strategies and CLI

1. Implement strategy functions: `strip_reasoning`, `strip_tools`.
   Each produces a `Compaction` event.
2. Implement range bound resolution (negative integers, duration strings, `last`
   → absolute turn index).
3. Add the `jp conversation compact` command with `--keep-first`, `--keep-last`,
   `--from`, `--to`, `--reasoning`, `--tools`, `--dry-run`, `--reset`.
4. Add `--compact` / `-k` to `jp conversation fork`.
5. Add `--compacted` to `jp conversation print`.
6. Add integration tests.

Depends on Phase 2.

### Phase 4: Configuration and DSL

1. Add `conversation.compaction` config section with `rules` as
   `MergedVec<CompactionRuleConfig>`.
2. Implement built-in default rule with `discard_when_merged: true`.
3. Implement the `--compact[=SPEC]` DSL parser.
4. Wire `--compact` / `-k` into `query`, `fork`, and `compact` with composition
   semantics (bare `--compact` = config rules, `--compact=SPEC` = DSL rule, both
   compose).
5. Add config and DSL tests.

Depends on Phase 3.
Can be partially parallelized with Phase 3 (config types and DSL parser can be
built before the CLI is wired up).

### Phase 5: LLM-Assisted Summarization

1. Implement the `summarize` strategy: read raw events, call the configured
   model, produce `SummaryPolicy { summary }`.
2. Implement the summary overlap auto-extension logic.
3. Add integration tests (with mock LLM).

Depends on Phase 2.
Can proceed in parallel with Phases 3 and 4.

## References

- [RFD 011]: System Notification Queue (compaction interaction)
- [RFD 034]: Inquiry-Specific Assistant Configuration (defers compaction)
- [RFD 036]: Conversation Compaction (superseded by this RFD)
- [Issue #57]: Make conversation management more powerful
- [Multi-turn degradation paper][paper]: cited in Issue #57

[Indexing and Counting Conventions]: ../wiki/00dxyba-indexing-and-counting-conventions.md
[InternalEvent]: https://github.com/dcdpr/jp/blob/main/crates/jp_conversation/src/stream.rs
[Issue #57]: https://github.com/dcdpr/jp/issues/57
[RFD 011]: 011-system-notification-queue.md
[RFD 034]: 034-inquiry-specific-assistant-configuration.md
[RFD 036]: 036-conversation-compaction.md
[anthropic-compaction]: https://docs.anthropic.com/en/docs/build-with-claude/compaction.md
[openai-compaction]: https://developers.openai.com/api/docs/guides/compaction.md
[paper]: https://arxiv.org/abs/2505.06120
