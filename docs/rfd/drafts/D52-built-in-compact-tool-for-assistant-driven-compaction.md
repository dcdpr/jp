# RFD D52: Built-in compact tool for assistant-driven compaction

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-04
- **Extends**: [RFD 064]

## Summary

Add a built-in `compact` tool that lets the assistant propose compaction rules
for the conversation it is having.
Each proposed rule carries the assistant's own summary text and a one-sentence
note, is approved by the user before it is recorded, and takes effect from the
next turn rather than immediately.
The tool composes existing machinery: [RFD 064]'s compaction events and
projection, the tool run-mode approval flow, and the built-in tool registry.

## Motivation

Compaction today is entirely user-driven.
The user notices the conversation is getting long, decides what is worth
keeping, and runs `jp conversation compact` with a range.
Deciding *what* to keep is the hard part, and the user is the participant worst
placed to do it: the relevant turns have scrolled out of the terminal, and
reconstructing them means `jp conversation edit --events` and reading raw JSON.

The assistant is better placed.
It holds the whole conversation in its context, it knows which parts are
recoverable from files on disk and which only exist in the thread, and it can
name the boundary precisely because it can quote the text.
In practice this already happens by hand: the user asks the assistant which
range to compact and what the summary should say, then translates the answer
into flags.
The tool removes the translation step, so `jp q -u compact "summarize everything
except the current task"` gets there in one command.

[RFD 064] provides everything needed to record the result.
What is missing is a way for a tool to propose one.

## Design

### What the assistant calls

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

Each rule accepts:

| Field          | Type   | Meaning                                               |
| -------------- | ------ | ----------------------------------------------------- |
| `keep_first`   | int    | Leave this many opening turns untouched.              |
| `keep_last`    | int    | Leave this many recent turns untouched. Minimum 1.    |
| `before_quote` | string | Range ends at the turn before the quoted one.         |
| `after_quote`  | string | Range starts at the turn after the quoted one.        |
| `reasoning`    | enum   | `strip`.                                              |
| `tool_calls`   | enum   | `strip`, `strip-requests`, `strip-responses`, `omit`. |
| `summary`      | string | Replace the range with this text.                     |
| `note`         | string | One sentence on why. Required.                        |

The range and policy fields mirror `CompactionRuleConfig`, so the tool speaks
the same vocabulary as `conversation.compaction.rules` and `jp conversation
compact`.
`summary` is a plain string rather than the config's `summary` block, which
makes it impossible to request a generated summary (see [Non-Goals]).

`keep_last` is clamped to a minimum of 1 by the tool.
Without the clamp the assistant can summarize away the turn in progress,
including the user's request and the tool call proposing the compaction.

### Naming a range by quote

The assistant cannot see turn numbers.
Nothing in the thread labels turns, so relative counts (`keep_last`) are the
only bound it can express reliably, and it can miscount.
It is good at reproducing text it was shown, so `before_quote` and `after_quote`
take a verbatim excerpt and JP resolves it to a turn:

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

Resolution runs against the *projected* stream, because that is what the
assistant was shown, and maps back to a raw turn index through the
`Vec<TurnOrigin>` that `apply_projection` already returns.
A quote landing inside a summary turn is `AlreadyCompacted` rather than silently
expanding to that summary's range.

`NotFound` and `Ambiguous` come back as tool errors listing candidate turns, so
the assistant corrects itself without involving the user.

The parameters are named for what the assistant supplies (a quote); `anchor` is
the internal term for what one resolves to.

Quote anchors are not a `RuleBound` variant and are not exposed on the CLI.
`RuleBound` parses from a string by sigil (`@N`, `-N`, `5h`, `last-compaction`),
and free text has no sigil, so a quote bound would need a table form
(`keep_first = { quote = "..." }`) that nothing needs yet.

### Approval and preview

The tool ships with `run = "ask"` and `format = "unattended"`, so its argument
formatter runs before the permission prompt and the user decides against the
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

The preview must be self-sufficient.
The user is mid-`jp q`, and the turns under discussion have scrolled away, so
resolved ranges, boundary excerpts, and the note all appear inline, and the full
set of dropped content spills to a temp file the user can open.
Spilling to a temp file is what the existing timeline rendering already does for
summaries.

Formatting the arguments needs the stream, which built-in tools cannot reach
today, so `BuiltinTool` gains two things:

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

`format_arguments` is a new value on the existing argument-formatter axis
(`json`, `function_call`, `off`, custom command), available to every built-in
rather than to this one.

`run = "edit"` remains the correction path: the user edits the proposed rules,
including the summary text, before the tool runs.

### Recording the result

A tool cannot mutate the conversation stream from inside `execute`: executors
run concurrently and `ConversationMut` is not shareable across them.
So the tool returns the mutation as a value and the turn loop applies it:

```rust
pub struct BuiltinOutcome {
    pub outcome: Outcome,
    pub effects: Vec<HostEffect>,
}

pub enum HostEffect {
    Compact(Vec<CompactionRuleConfig>),
}
```

Effects travel on `jp_llm`'s internal `ExecutionOutcome` and `ExecutorResult`,
never on `jp_tool::Outcome`.
`jp_tool::Outcome` is the external local-tool protocol, and mutating the
conversation stream is a host-only capability that local tools must not be able
to express.

The turn loop applies effects in `commit_tool_responses`, inside the
`update_events` scope that already commits the tool responses, so the response
and the compaction land together and flush together.
Multiple effects apply in tool-call order, each planned against the stream as
mutated so far, which is how overlapping rules from concurrent calls get
resolved by the existing overlap logic.

Applying an effect needs no provider call in this design, so the whole path is
synchronous and the planner is the pure one extracted in Phase 1.

### Deferred activation

A compaction recorded during a turn does not affect that turn.
`Compaction` gains an activation field:

```rust
pub struct Compaction {
    pub from_turn: usize,                   // covered range
    pub to_turn: usize,                     // covered range
    pub deferred_until_turn: Option<usize>, // activation
    // ...
}
```

`None` means no deferral, which is every existing event and everything `jp
conversation compact` writes.
`Some(n)` means projection skips the compaction until the stream's highest turn
index reaches `n`.
The tool writes `Some(turn_count)`, so the compaction activates when the next
turn starts.

`apply_projection` already computes the highest turn index, and the check is a
comparison against it, so the projection stays a pure function of the stream.

This buys the assistant free iteration.
Within a turn it can compact, see the result, decide the range was wrong, and
correct itself, and none of it rewrites the prompt prefix.
The rest of the turn runs on the same cached prefix, and the whole accumulated
set takes effect at once when the user sends their next message.

Recording immediately rather than staging in memory also means the stream stays
the single source of truth.
There is no in-memory set to reconcile, a crash leaves the recorded compactions
intact and simply activates them next turn, and `--replay` re-walks the same
events.
The tool's own preview and overlap checks read `stream.compactions()`, which
already contains deferred entries, so no extra plumbing is needed to make a
second call see the first.

### Attribution, provenance, and notes

`Compaction` gains two more fields:

```rust
pub enum CompactionSource { User, Assistant, Auto }

pub struct Compaction {
    pub source: CompactionSource,  // serde default: User
    pub note: Option<String>,
    // ...
}
```

`source` distinguishes what the user asked for from what the assistant proposed,
which `jp conversation print` surfaces and a future source-filtered reset keys
on.
It does not drive activation; `deferred_until_turn` does that independently, so
an automatic trigger can activate immediately without touching attribution.

`note` holds the one-sentence explanation.
`jp conversation compact --note TEXT` gives the user the same field for their
own compactions.
The tool truncates at 200 characters rather than erroring.

`SummarySource` is renamed to name the property the overlap logic actually keys
on, which is whether JP can produce the text again for a different range:

```rust
pub enum SummarySource {
    Derived,                      // JP holds the recipe and can re-derive
    Supplied { author: Author },  // handed to JP; not reproducible
}

pub enum Author { User, Assistant }
```

`Generated` and `Authored` describe token provenance, which is the wrong axis: a
summary the assistant typed into a tool call is produced by a model but is no
more re-derivable than one the user typed, because JP holds no recipe for it.
Widening it would leave the assistant's words standing for turns they never
described.
`extend_summary_range` keys on `is_re_derivable()`, which is a match on the
outer discriminant, so a later `Author` variant cannot silently change the
overlap rules.

### Enabling the tool

`enable = { state: false, allow_toggle: IfNamed }`, the `"explicit"` shorthand.
The tool is never available unless named: `jp q -t compact` enables it, and `jp
q -u compact` forces its use.
No new activation machinery is involved.

## Drawbacks

- **No relief within the turn that asks for it.** An assistant approaching its
  context ceiling mid-turn gets nothing until the turn ends.
  The assistant cannot see token counts, so it cannot reliably detect that case
  anyway; a trigger with real numbers is the right owner.
- **Range quality rests on the preview.** The assistant chooses the range and
  the user's only check is the approval prompt.
  If the preview is not self-sufficient, the user approves something they have
  not read.
- **A quote can miss.** Ambiguous or unmatched anchors cost a round trip, and a
  paraphrased quote fails.
- **One prefix rebuild per turn boundary.** The turn after activation pays a
  prompt-cache miss.
  That is the inherent cost of compacting, and deferral collapses any number of
  within-turn corrections into a single miss.
- **Three turn-valued concepts on one type.** `from_turn`, `to_turn`, and
  `deferred_until_turn` mean different things.
  The naming keeps them apart, but a reader has to learn the distinction.
- **`deferred_until_turn` is position-based.** A hand-edit that deletes turns
  can shift a compaction back out of activation.
- **Assistant-selectable ranges reach user-facing content.** Tool-call policies
  drop request and response pairs wholesale, which matters once tools carry
  user-addressed messages.
  The raw events survive, so this is view fidelity rather than data loss.

## Alternatives

**Activate at the cycle boundary.** Apply each compaction after the cycle that
proposed it.
Cheaper for a single compaction, since the remaining cycles run on a smaller
context.
Rejected because the interesting correction arrives one cycle later, after the
assistant has seen the result, and by then the rewrite is already paid.
Under iteration, N corrections cost N prefix rebuilds instead of one.

**Stage the set in memory and apply at turn end.** Same user-visible behavior as
deferred activation.
Rejected because it creates two truths for the length of a turn, needs lifecycle
rules for interrupts and restarts, and loses the set on a crash.
Recording a deferred event gets the same behavior with one source of truth.

**Derive the applied set from the `compact` tool calls at turn end.**
Attractive, because the requests are already in the stream.
Rejected because approval state is not recoverable: a declined or skipped call
still has its arguments in the request, and only the `Result<String, String>`
response distinguishes it, which would mean matching on synthesized skip
messages.

**Positional activation** ("a compaction after the last `TurnStart` is
inactive").
No new field, but `jp conversation compact` run between turns also lands after
the last `TurnStart`, so it would stop taking effect immediately.

**Activation derived from `source`** ("assistant compactions are inactive until
the next turn").
Avoids the field but makes activation implicit and entangles it with
attribution, which forecloses an automatic trigger choosing to activate
immediately.

**Expose turn numbers to the assistant.** A turn map in the system prompt would
let the assistant name indices directly.
Rejected: it changes every turn, so it destroys the system-prompt cache.

**Confirm the resolved range with the assistant** via a tool question before
asking the user.
Deferred: the user's approval already gates the result, and the round trip costs
a cycle.

## Non-Goals

- **Generated summaries.** The assistant cannot ask JP to summarize a range for
  it.
  The mechanism is nearly free, since the effect carries rules and the existing
  pipeline knows how to generate, but the design questions are not: the user
  would approve a range whose replacement text does not exist yet, the assistant
  never sees the text that now stands for its own history unless JP echoes it
  back at token cost, and a summarizer failure after approval needs a policy.
  The feature is useful without it.
- **Undoing a compaction.** `compact(reset:)` is deliberately absent.
  Resetting grows the working context and discards summary text that cost money,
  so an assistant able to compact and un-compact in one turn can thrash in both
  directions.
  If it lands later it needs its own user approval.
  Recovering earlier detail belongs on a read tool, not on the compaction axis.
- **Within-turn context relief.** Acting when the context is genuinely near the
  ceiling needs a token-count trigger, which the automatic-compaction design
  owns.
  It can reuse this RFD's planner and effect machinery.
- **Quote anchors on the CLI.** Internal to the tool for now.
- **A schema-level length cap on `note`.** `ToolParameterConfig` has no
  `maxLength`, and adding one is a config-surface change worth doing on its own.
  Host-side truncation is the guarantee regardless, because providers only
  enforce tool schemas in strict modes.

## Risks and Open Questions

- **How reliably does the assistant quote?** The anchor design assumes it
  reproduces text verbatim rather than paraphrasing.
  Unvalidated until the tool is in use.
  Relative bounds are the fallback, and both fail visibly at approval.
- **Is the preview enough?** The design assumes resolved ranges, excerpts, and a
  spilled preview file let the user approve safely without leaving the terminal.
  If it is not, the next lever is fewer rules per call, not more chrome.
- **Should deferred compactions be inspectable mid-turn?** They are already
  persisted, so a second terminal can see them, but there is no dedicated view.
- **Does `--from last-compaction` want to see deferred entries?** It does today
  by construction, since `stream.compactions()` includes them, which is the
  behavior that avoids re-compacting a claimed range.
  Worth confirming it reads correctly for a user running `jp conversation
  compact` between turns.

## Implementation Plan

The `-u NAME` implies `-t NAME` fix and the built-in config merge-under fix are
prerequisites and land separately; both are generic and benefit every built-in.

### Phase 1: Extract the planner

Move the pure planning functions (`plan_compactions`, `resolve_rule_range`,
`build_mechanical_compaction`, and the `RuleBound` to `RangeBound` mapping) from
`jp_cli::cmd::conversation::compact` into `jp_conversation::compaction`.
Summary generation stays in `jp_cli`, which keeps the extracted core
synchronous.
Behavior-preserving, mergeable on its own, and shared by three callers.

### Phase 2: Compaction event fields

Add `deferred_until_turn`, `source`, and `note` to `Compaction`, with the
projection check for activation.
Rename `SummarySource` to `Derived` / `Supplied { author }` with the
`is_re_derivable` predicate.
Add `--note` to `jp conversation compact`.
No backward-compatibility shim for the old `SummarySource` strings: the field is
unreleased and only exists in local development data.
Mergeable on its own, no behavioral change until Phase 4.

Depends on the verbatim-summary work ([PR 932]) landing first, which introduces
the type being renamed.

### Phase 3: Built-in host effects

Add `BuiltinContext`, `BuiltinOutcome`, `HostEffect`, and the `format_arguments`
hook to `BuiltinTool`.
Thread effects through `ExecutionOutcome` and `ExecutorResult` to the
coordinator, and apply them in `commit_tool_responses` inside the existing
`update_events` scope.
Migrate `describe_tools` to the widened trait.
Mergeable with no user-visible change, since nothing emits an effect yet.

### Phase 4: Anchors and the tool

Add `resolve_anchor` and `AnchorError` to `jp_conversation::compaction`.
Add the `compact` built-in: argument parsing and validation, the `keep_last`
clamp, the formatter with excerpts and the spilled preview file, and the config
entry in `builtins::all()`.

Tests: anchor resolution against a projected stream including the
already-compacted case, the `keep_last` clamp, deferred activation across a turn
boundary, two calls in one turn where the second overlaps the first, and a
declined call recording nothing.

Depends on Phases 1 through 3.

## References

- [RFD 064] — non-destructive compaction; this RFD adds a second author of
  compaction events.
- [RFD 081] — the `enable = { state, allow_toggle }` shape this tool uses.
- [RFD 083] — the built-in registration pattern and the `if_named` reasoning.
- [RFD 078] — names the missing built-in `Context` that Phase 3 introduces, and
  is the natural second consumer of the effect channel.
- [RFD 097] — stable entry identifiers, the general fix for position-based
  references like `deferred_until_turn`.
- [PR 932] — verbatim summaries and `SummarySource`.

[Non-Goals]: #non-goals
[PR 932]: https://github.com/dcdpr/jp/pull/932
[RFD 064]: ../064-non-destructive-conversation-compaction.md
[RFD 078]: ../078-tool-config-mutation.md
[RFD 081]: ../081-decompose-tool-enable-into-state-and-allow_toggle.md
[RFD 083]: ../083-built-in-ask_user-tool-for-assistant-initiated-inquiries.md
[RFD 097]: ../097-stable-event-identifiers.md
