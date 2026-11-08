# RFD D69: Built-in compact Tool for Assistant-Driven Compaction

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-04
- **Extends**: [RFD 064]
- **Summary**: Adds a built-in `compact` tool for the assistant to propose
  user-approved compaction rules that apply on the next turn.

## Summary

This RFD adds a built-in `compact` tool that lets the assistant propose
compaction rules for the active conversation.
Each rule includes a one-sentence rationale and optional summary text, requires
user approval before it is recorded, and takes effect on the next turn through
[RFD 064]'s compaction overlay and projection.

## Motivation

Compaction in [RFD 064] is entirely user-driven.
When a conversation grows long, the user decides which turns to keep and runs
`jp conversation compact` with a turn range.
Choosing that range is difficult for the user because the relevant turns have
usually scrolled out of the terminal, and inspecting them requires running `jp
conversation edit --events` and reading raw JSON.

The assistant already holds the full conversation in context.
It knows which details can be recovered from files on disk and which exist only
in the thread, and it can mark turn boundaries by quoting the text.
Users already delegate this step manually by asking the assistant which range to
compact and what the summary should say, then translating the response into CLI
flags.
A built-in `compact` tool removes that translation step so `jp q -u compact
"summarize everything except the current task"` completes the workflow in one
command.

[RFD 064] already defines how compaction events are recorded and projected; this
RFD adds a way for a tool to propose them.

## Design

### Tool Interface

The `compact` tool accepts a list of compaction rules:

```json
{
  "name": "compact",
  "arguments": {
    "rules": [
      {
        "before_quote": "so the divider cursor works if I move to the divider",
        "summary": "We fixed the container-width bug, settled on the drag verb, ...",
        "note": "Everything earlier is recoverable from files on disk."
      },
      {
        "keep_last": 4,
        "reasoning": "strip",
        "note": "The thinking blocks are long and their conclusions are in the text."
      }
    ]
  }
}
```

Each rule accepts the following fields:

| Field          | Type   | Meaning                                               |
| -------------- | ------ | ----------------------------------------------------- |
| `keep_first`   | int    | Leave this many opening turns untouched.              |
| `keep_last`    | int    | Leave this many recent turns untouched. Minimum 1.    |
| `before_quote` | string | End the range at the turn before the quoted turn.     |
| `after_quote`  | string | Start the range at the turn after the quoted turn.    |
| `reasoning`    | enum   | `strip`.                                              |
| `tool_calls`   | enum   | `strip`, `strip-requests`, `strip-responses`, `omit`. |
| `summary`      | string | Replace the range with this text.                     |
| `note`         | string | One-sentence rationale for the rule. Required.        |

The range and policy fields mirror `CompactionRuleConfig`, matching the
vocabulary of `conversation.compaction.rules` and `jp conversation compact`.
`summary` is a plain string rather than the config's `summary` table, so the
tool cannot request a generated summary (see [Non-Goals]).

The tool clamps `keep_last` to a minimum of 1 so the assistant cannot compact
the active turn, which contains the user's prompt and the `compact` call itself.

### Resolving Ranges by Quote

Because turns are not numbered in the conversation thread, the assistant cannot
reference turn indices directly, and relative counts (`keep_first`, `keep_last`)
are easy to miscount.
However, the assistant can quote text from its context window verbatim.
`before_quote` and `after_quote` accept a verbatim excerpt that JP resolves to a
turn index:

```rust
// jp_conversation::compaction
pub fn resolve_anchor(
    events: &ConversationStream,
    quote: &str,
) -> Result<usize, AnchorError>;

pub enum AnchorError {
    NotFound,
    Ambiguous { turns: Vec<usize> },
    AlreadyCompacted { turn: usize },
}
```

Anchor resolution searches the *projected* stream (the view sent to the
assistant) and maps the matched turn back to a raw turn index through the
`Vec<TurnOrigin>` returned by `apply_projection`.
If a quote matches a summary turn from an earlier compaction, `resolve_anchor`
returns `AnchorError::AlreadyCompacted` rather than silently expanding to that
summary's underlying turn range.

`NotFound` and `Ambiguous` return tool errors that list candidate turns,
allowing the assistant to retry with a more specific quote without prompting the
user.

The tool parameters (`before_quote`, `after_quote`) name the excerpt the
assistant supplies; an **anchor** is the resolved turn boundary in
`jp_conversation::compaction`.

Quote anchors are not a `RuleBound` variant and are not exposed on the CLI.
`RuleBound` values parse from sigil-prefixed strings (`@N`, `-N`, `5h`,
`last-compaction`); supporting arbitrary text would require a table syntax
(`keep_first = { quote = "..." }`) with no current CLI or config use case.

### Approval and Preview

The tool is configured with `run = "ask"` and `format = "unattended"`, so its
argument formatter runs before the permission prompt and the user reviews the
resolved plan rather than raw JSON:

```
compact — 2 rules proposed

  1. turns 1–14 → your summary (180 words), ~11k tokens smaller
     from  "so the divider cursor works if I move to the divider from…"
     to    "That keeps the whole live thread verbatim: the one-sided-drag…"
     why   Everything earlier is recoverable from files on disk.
     preview  /tmp/jp-<conv>-compact-1.md

  2. turns 15–18 → strip reasoning
     why   The thinking blocks are long and their conclusions are in the text.

Run compact? [y/n/e/?]
```

Because the target turns have usually scrolled off-screen during `jp q`, the
preview must stand on its own.
It prints the resolved turn ranges, boundary excerpts, and rationale inline, and
writes the full dropped content and replacement summary to a temporary file
(matching how timeline rendering spills long summaries).

Resolving quotes and rendering boundary excerpts require access to the
conversation stream.
`BuiltinTool` therefore takes a `BuiltinContext` and adds an optional
`format_arguments` method:

```rust
pub trait BuiltinTool {
    async fn execute(
        &self,
        args: &Value,
        answers: &IndexMap<String, Value>,
        ctx: &BuiltinContext,
    ) -> BuiltinOutcome;

    fn format_arguments(&self, args: &Value, ctx: &BuiltinContext) -> Option<String> {
        None
    }
}

pub struct BuiltinContext {
    pub events: ConversationStream,
}
```

`format_arguments` plugs into the existing argument-formatter pipeline alongside
`json`, `function_call`, `off`, and custom formatter commands, and is available
to any built-in tool.

Choosing edit (`run = "edit"`) opens the proposed rules in the user's editor so
they can adjust the bounds or summary text before the tool executes.

### Recording Compactions via Host Effects

A built-in tool cannot mutate the conversation stream inside `execute` because
tool executors run concurrently and `ConversationMut` cannot be shared across
them.
Instead, `execute` returns stream mutations as `HostEffect` values for the turn
loop to apply:

```rust
pub struct BuiltinOutcome {
    pub outcome: Outcome,
    pub effects: Vec<HostEffect>,
}

pub enum HostEffect {
    Compact(Vec<CompactionRuleConfig>),
}
```

`HostEffect` values travel on `jp_llm`'s internal `ExecutionOutcome` and
`ExecutorResult` types, never on `jp_tool::Outcome`.
`jp_tool::Outcome` defines the external local-tool protocol, and mutating the
conversation stream is a host-only capability that external tools must not be
able to emit.

The turn loop applies effects in `commit_tool_responses` inside the
`update_events` scope that records tool responses, so the tool response and its
compaction events are committed and flushed atomically.
When multiple tool calls emit effects in the same cycle, they are applied in
tool-call order; each effect is planned against the stream mutated by earlier
effects, allowing the existing overlap logic to resolve concurrent rules.

Because rules carry pre-written summaries, applying a `Compact` effect requires
no LLM provider call.
The commit path stays synchronous and uses the pure planner extracted in Phase
1.

### Deferred Activation

A compaction recorded during a turn does not take effect until that turn
finishes.
`Compaction` gains an activation threshold field:

```rust
pub struct Compaction {
    pub from_turn: usize,                   // covered range
    pub to_turn: usize,                     // covered range
    pub deferred_until_turn: Option<usize>, // activation
    // ...
}
```

`None` activates the compaction immediately; all existing compaction events and
those created by `jp conversation compact` use `None`.
`Some(n)` instructs `apply_projection` to skip the compaction until the stream's
highest turn index reaches `n`.
The `compact` tool writes `Some(turn_count)`, activating the compaction when the
next turn starts.

Because `apply_projection` already computes the stream's highest turn index,
checking `deferred_until_turn` keeps projection a pure function of the stream.

Deferring activation lets the assistant iterate within a turn without
invalidating the prompt cache.
It can propose a compaction, inspect the outcome, and submit a corrective rule
while the remaining tool cycles continue using the cached prompt prefix.
All accumulated compactions take effect together on the next turn.

Persisting deferred events immediately keeps the conversation stream as the
single source of truth.
No in-memory staging state needs reconciliation: a crash leaves the recorded
compactions on disk to activate on the next turn, and `--replay` replays the
same event sequence.
Because `stream.compactions()` includes deferred entries, subsequent `compact`
calls in the same turn automatically see earlier ones during preview and overlap
resolution.

### Attribution, Provenance, and Notes

`Compaction` gains two metadata fields:

```rust
pub enum CompactionSource { User, Assistant, Auto }

pub struct Compaction {
    pub source: CompactionSource,  // serde default: User
    pub note: Option<String>,
    // ...
}
```

`source` records who initiated the compaction (`User`, `Assistant`, or `Auto`)
for `jp conversation print` and future source-filtered resets.
Attribution is independent of activation (`deferred_until_turn`), so an `Auto`
trigger can activate immediately while preserving its source.

`note` stores the one-sentence rationale.
`jp conversation compact --note TEXT` exposes the same field on the CLI.
The `compact` tool truncates `note` at 200 characters instead of rejecting long
values.

The `SummarySource` variants are renamed to reflect what the overlap logic
actually depends on: whether JP can regenerate the summary for a wider range:

```rust
pub enum SummarySource {
    Derived,                      // JP holds the recipe and can re-derive
    Supplied { author: Author },  // handed to JP; not reproducible
}

pub enum Author { User, Assistant }
```

The previous variant names (`Generated` and `Authored`) described who wrote the
tokens, which is the wrong distinction.
A summary passed by the assistant to `compact` is model-generated, but JP cannot
regenerate it for a different turn range any more than it can regenerate a
user-written summary; widening that compaction's range would leave the
assistant's summary covering turns it never described.
`extend_summary_range` checks `is_re_derivable()`, which matches only on the
outer `SummarySource` variant so adding future `Author` variants cannot silently
alter overlap behavior.

### Enabling the Tool

The `compact` tool is configured with `enable = { state: false, allow_toggle:
IfNamed }` (the `"explicit"` shorthand).
It is disabled by default and available only when named on the CLI: `jp q -t
compact` makes it available to the assistant, and `jp q -u compact` forces a
call.
This uses the existing tool-activation mechanism with no new machinery.

## Drawbacks

- **No context reduction during the current turn.** Compactions do not shrink
  the prompt until the next turn starts.
  However, because the assistant cannot see token counts, it cannot reliably
  detect context-window pressure mid-turn anyway; automatic compaction triggered
  by actual token counts is better suited to that case.
- **Range quality depends on the approval preview.** Because the assistant
  selects the turn range, the permission prompt is the user's only safeguard.
  If the preview lacks context, the user may approve a compaction without
  realizing what it drops.
- **Quote matching can fail.** Paraphrased, unmatched, or ambiguous quotes fail
  anchor resolution and cost an extra tool round trip.
- **Prompt-cache miss on the next turn.** Activating a compaction rewrites the
  conversation history and invalidates the cached prompt prefix on the following
  turn.
  This cost is inherent to compaction, and deferral collapses multiple
  within-turn adjustments into a single cache miss.
- **Three turn-index fields on `Compaction`.** `from_turn` and `to_turn` bound
  the compacted range, while `deferred_until_turn` gates when the compaction
  becomes active.
- **`deferred_until_turn` is position-based.** Deleting turns via manual stream
  edits can lower the total turn count below `deferred_until_turn` and
  deactivate an already-active compaction.
- **Tool-call compaction can hide user-facing tool output.** Tool-call policies
  strip request and response pairs wholesale, including any user-addressed
  messages carried by those tools.
  Because raw events remain in the stream, this affects only the projected view,
  not persisted data.

## Alternatives

**Activate at the cycle boundary.** Applying each compaction immediately after
the tool cycle that proposed it would let remaining cycles in the same turn run
on a smaller context.
However, the assistant only sees the tool result on the next cycle, when the
prompt prefix has already been rewritten; N iterative corrections would cause N
prompt-cache misses instead of one.

**Stage compactions in memory until the turn ends.** Holding proposed
compactions in memory produces the same user-visible behavior as deferred
activation, but splits state between memory and the stream during the turn,
requires custom handling for interrupts and restarts, and loses staged
compactions on a crash.
Persisting a deferred event achieves the same result with a single source of
truth.

**Derive compactions from `compact` tool calls at turn end.** Reconstructing
rules from `ToolCallRequest` events at the end of the turn avoids `HostEffect`,
because the requests are already in the stream.
However, the stream does not record structured approval state: declined and
skipped calls still appear as requests, and distinguishing them from approved
calls would require parsing synthesized skip messages out of the `Result<String,
String>` tool response.

**Positional activation.** Treating any `Compaction` event after the last
`TurnStart` as inactive avoids adding `deferred_until_turn`.
However, `jp conversation compact` run between turns also appends events after
the last `TurnStart`, so CLI compactions would stop taking effect immediately.

**Derive activation from `source`.** Treating `CompactionSource::Assistant`
events as inactive until the next turn avoids `deferred_until_turn`, but couples
activation timing to attribution and prevents `Auto` compactions from choosing
per-event whether to activate immediately or defer.

**Expose turn numbers to the assistant.** Injecting a turn index map into the
system prompt would let the assistant specify turn numbers directly, but
updating that map on every turn would invalidate the system-prompt cache.

**Confirm the resolved range with the assistant before prompting the user.**
Having the tool ask the assistant to confirm the resolved turn numbers via a
tool inquiry before showing the user approval prompt would catch bad ranges
earlier, but user approval already gates execution and the extra round trip
costs a full model cycle.

## Non-Goals

- **Generated summaries.** The assistant must supply the summary text itself
  rather than asking JP to generate one.
  While the compaction pipeline already supports summary generation, exposing it
  to the tool raises unresolved UX and failure questions: the user would have to
  approve a compaction before seeing its summary, the assistant would not know
  how its own history was summarized unless JP echoed the generated text back at
  extra token cost, and post-approval summarization failures would need
  error-recovery rules.
- **Undoing compactions.** The tool omits a `compact(reset:)` action.
  Resetting a compaction expands the context window and discards summaries that
  cost tokens to produce; allowing the assistant to compact and un-compact
  within a turn risks thrashing.
  Inspecting compacted turns belongs in a read-only history tool rather than
  compaction reset.
- **Within-turn context relief.** Shrinking a prompt that approaches the context
  limit mid-turn requires a token-count trigger, which belongs to automatic
  compaction and can reuse the planner and `HostEffect` pipeline introduced
  here.
- **Quote anchors on the CLI.** Quote-based bounds (`before_quote`,
  `after_quote`) are restricted to the `compact` tool.
- **A schema-level length cap on `note`.** `ToolParameterConfig` does not
  support `maxLength`, and adding schema validation constraints is a separate
  config change.
  The host must truncate `note` regardless because LLM providers only enforce
  string length limits in strict schema modes.

## Risks and Open Questions

- **How reliably does the assistant quote verbatim?** Quote anchors assume the
  model copies text verbatim rather than paraphrasing, which remains unvalidated
  until the tool sees real use.
  Relative bounds (`keep_first`, `keep_last`) provide a fallback, and any
  misresolved bound is visible in the approval preview.
- **Is the approval preview sufficient?** This design assumes that inline turn
  ranges, boundary excerpts, and a temporary preview file give the user enough
  context to approve rules safely from the terminal.
  If multi-rule previews prove too dense to review, limiting the tool to fewer
  rules per call is preferable to adding more terminal UI.
- **Should deferred compactions have a dedicated mid-turn view?** Deferred
  compactions are persisted immediately and visible to other processes reading
  the stream, but no CLI view explicitly labels them as pending activation.
- **Should `--from last-compaction` include deferred compactions?** Because
  `stream.compactions()` returns all persisted compactions, `--from
  last-compaction` includes deferred entries by default.
  That prevents a second `compact` call in the same turn from re-compacting an
  already-claimed range, and also applies if the user runs `jp conversation
  compact` before the next turn starts.

## Implementation Plan

Two generic fixes land separately as prerequisites: making `-u NAME` imply `-t
NAME`, and merging built-in tool defaults under user config.

### Phase 1: Extract the Planner

Move the pure planning functions (`plan_compactions`, `resolve_rule_range`,
`build_mechanical_compaction`, and the `RuleBound`-to-`RangeBound` mapping) from
`jp_cli::cmd::conversation::compact` into `jp_conversation::compaction`.
Summary generation remains in `jp_cli` so the extracted planner stays
synchronous.
This refactoring preserves existing behavior, can merge independently, and
shares the planner across all three compaction entry points.

### Phase 2: Compaction Event Fields

Add `deferred_until_turn`, `source`, and `note` to `Compaction`, and update
`apply_projection` to skip compactions whose `deferred_until_turn` threshold has
not been reached.
Replace the `SummarySource` variants with `Derived` and `Supplied { author }`,
and add the `is_re_derivable` predicate.
Add `--note` to `jp conversation compact`.
No serde compatibility shim is needed for the old `SummarySource` variant names
because the field is unreleased and exists only in local development data.

This phase depends on [PR 932] (which introduces `SummarySource`) and can merge
independently with no behavioral change until Phase 4.

### Phase 3: Built-in Host Effects

Add `BuiltinContext`, `BuiltinOutcome`, `HostEffect`, and `format_arguments` to
`BuiltinTool`, and update `describe_tools` for the new trait signature.
Thread `HostEffect` values through `ExecutionOutcome` and `ExecutorResult` to
the turn coordinator, and apply them in `commit_tool_responses` inside the
existing `update_events` scope.
This phase can merge independently with no user-visible change.

### Phase 4: Anchors and the Tool

Add `resolve_anchor` and `AnchorError` to `jp_conversation::compaction`.
Implement the `compact` built-in tool: argument parsing and validation, the
`keep_last` minimum clamp, the argument formatter with inline boundary excerpts
and temporary preview file, and the default tool entry in `builtins::all()`.

Add tests covering anchor resolution against a projected stream (including
`AlreadyCompacted`), the `keep_last` clamp, deferred activation across a turn
boundary, overlapping rules across two `compact` calls in the same turn, and
declined tool calls leaving the stream unchanged.

Depends on Phases 1 through 3.

## References

- [RFD 064]: Non-destructive conversation compaction, which this RFD extends
  with assistant-proposed compaction events.
- [RFD 081]: The `enable = { state, allow_toggle }` configuration shape used by
  `compact`.
- [RFD 083]: Built-in tool registration and the rationale for `IfNamed`
  activation.
- [RFD 078]: Tool configuration mutation, which identified the need for
  `BuiltinContext` and can share the `HostEffect` channel introduced in Phase 3.
- [RFD 097]: Stable event identifiers, which replace position-based references
  such as `deferred_until_turn`.
- [PR 932]: Verbatim summaries and `SummarySource`.

[Non-Goals]: #non-goals
[PR 932]: https://github.com/dcdpr/jp/pull/932
[RFD 064]: ../064-non-destructive-conversation-compaction.md
[RFD 078]: ../078-tool-config-mutation.md
[RFD 081]: ../081-decompose-tool-enable-into-state-and-allow_toggle.md
[RFD 083]: ../083-built-in-ask_user-tool-for-assistant-initiated-inquiries.md
[RFD 097]: ../097-stable-event-identifiers.md
