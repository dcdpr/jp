# RFD D50: Automatic Compaction

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-15
- **Extends**: [RFD 064]

## Summary

This RFD extends [RFD 064] with automatic compaction: when a conversation's
projected size approaches the model's context window, JP appends a rolling
compaction event between turns, with no user action required.
Automatic compaction reuses the existing compaction event model and projection
layer without change, adding only a trigger and a standing rule.
It is disabled by default.

## Motivation

[RFD 064] delivers compaction as an explicit, user-initiated operation.
The user must notice that a conversation is getting long, decide to compact, and
run `jp conversation compact` at the right moment.
In long-running coding sessions the context window fills gradually, and quality
degrades (and cost rises) before the user intervenes.
The point of compaction is to keep the working context lean; requiring manual
vigilance undercuts that.

[RFD 064] anticipated this and reserved the `conversation.compaction.auto`
namespace for a follow-up.
This is that follow-up.

## Design

### What the user configures

Automatic compaction adds an `auto` section under `conversation.compaction`:

```toml
[conversation.compaction.auto]
enabled = false        # opt-in; off by default
trigger_ratio = 0.75   # fire when the projected size exceeds 75% of the window
min_turns = 5          # never auto-compact a conversation shorter than this

[conversation.compaction.auto.rule]
keep_first = 1
keep_last = 3
reasoning = "strip"
tool_calls = "strip"
```

`auto.rule` is a `CompactionRuleConfig`, the same type as the entries in
`conversation.compaction.rules`.
It defaults to the built-in mechanical policy (strip reasoning and tool calls,
keep first 1, keep last 3).
To make automatic compaction summarize instead, give the rule a `summary` block,
exactly as for a manual rule.

There is one rule, not a list (see Non-Goals).

### A materialized event, not an inference

When the trigger fires, automatic compaction appends a `Compaction` event to the
stream, identical in kind to what `jp conversation compact` produces.
It is **not** recomputed at projection time from the config and the turn count.

This is deliberate, for two reasons:

1. **Summaries are eager.** [RFD 064]'s eagerness principle stores generated
   summary text in the event because regenerating it is costly and
   non-deterministic.
   An inference model would have to regenerate the summary on every read (`jp
   query`, `jp conversation print --compacted`), which is untenable.
   Summaries must be materialized, so automatic compaction materializes.
2. **It keeps the projection layer pure.** The projection in
   `Thread::into_parts()` is a pure function of the stream alone.
   The trigger needs the model's context window and a size estimate, which are
   runtime concerns.
   Materializing the event in the turn loop keeps that decision in the
   imperative shell, where model details already live, and leaves the pure
   projection core untouched.

Automatic compaction is a new *trigger*, not a new compaction *mechanism*.
The projection layer does not change.

### Distinguishing automatic from manual compactions

The `Compaction` type gains a source marker:

```rust
pub enum CompactionSource {
    Manual,
    Auto,
}
```

Existing events deserialize as `Manual` (the serde default), so this is backward
compatible.
The marker lets `jp conversation print` show which overlays were automatic, and
lets a future selective reset target them.
It does not affect projection.

### The trigger

After a turn completes, before the stream is persisted, JP estimates the
projected size and compares it to the window:

```
estimated_tokens = projected_character_count / 4
threshold        = context_window * trigger_ratio
```

The estimate is taken over the *projected* view (after existing compactions are
applied), not the raw stream.
This matters: once a compaction has run, the projected size drops below the
threshold and the trigger goes quiet until the conversation grows again.
Estimating the raw stream would re-fire every turn forever, since overlay events
never shrink the raw history.

A firing happens when all of these hold:

- `auto.enabled` is true,
- the conversation has more than `min_turns` turns,
- `estimated_tokens > threshold`,
- the rolling range (below) is non-empty.

The trigger evaluates and fires at most once per turn boundary.
It never loops.

### The rolling range

The appended compaction covers:

```
from = max(keep_first, after_last_compaction)
to   = last_turn - keep_last
```

`keep_first` is a permanent floor: the opening turns are never compacted.
`after_last_compaction` is the turn following the furthest-compacted turn in the
stream (the existing `RangeBound::AfterLastCompaction`, already used by `--from
last`).
The larger of the two wins, so each firing compacts only turns no prior
compaction has touched.
`keep_last` protects the live tail.

Concretely, with `keep_first = 1` and `keep_last = 3`:

- After turn 12, with no prior compaction: compact turns `[1, 9]`.
  Turn 0 is preserved by `keep_first`; turns 10 through 12 by `keep_last`.
- After turn 20, with that first compaction ending at turn 9: compact turns
  `[10, 17]`.
  The bands do not overlap.

Because the bands never overlap, a summary covers each band exactly once,
generated from raw events, and [RFD 064]'s summary overlap auto-extension is
never triggered by automatic compaction.
The projected view accumulates sequential summary blocks followed by the live
tail.

### Context window discovery

The trigger needs `ModelDetails::context_window`, which the turn loop already
has via the active provider.
It is an `Option<u32>`: when the window is unknown (for example, a local model
without metadata), automatic compaction is skipped and logged at debug level
rather than failing.

### Behavior

On each turn boundary, when enabled:

1. Estimate the projected size and compare it to the threshold.
   If under, stop.
2. Check `min_turns`.
   If the conversation is too short, stop.
3. Resolve the rolling range.
   If empty, stop.
4. Build a `Compaction` (source `Auto`) from `auto.rule`.
   If the rule has a summary, generate it from the raw events in the range.
5. Append the event and persist.
   Log the range, the policy, and the estimated reduction.

### Prompt cache interaction

Appending a compaction changes the projected prefix, so the turn after automatic
compaction fires pays a one-time prompt-cache miss.
This is the inherent cost of compacting, not a defect: a single cache miss buys
a much smaller ongoing context.
The default `trigger_ratio` of 0.75 keeps firings infrequent, and the system
prompt cache (a separate prefix) is unaffected.
[RFD 064] flagged caching as the reason to keep compaction manual; the
mitigations here are the high threshold and the once-per-boundary cap.

## Drawbacks

- **Lossy and easy to miss.** A user may not realize the conversation was
  compacted.
  Mitigations: disabled by default, logged when it fires, marked `Auto` so
  `print` can surface it, and the raw events are always preserved.
- **Approximate trigger.** Character-based estimation can be off by a factor of
  two or three depending on content.
  `trigger_ratio` is the safety margin, and a tokenizer-based estimate can
  replace the heuristic later without changing the model.
- **Event accumulation.** A long session accumulates one auto event per firing.
  They are cheap metadata covering distinct, non-overlapping bands, and
  `--reset` clears them.
  Coalescing adjacent mechanical bands is a possible later optimization.
- **One-time cache invalidation per firing**, as above.

## Alternatives

### Config-inferred projection (no event)

Compute the compacted view at read time from the `auto` config plus the turn
count, storing nothing.
Rejected: it cannot store eager summaries (it would regenerate them on every
read), and it would push model-window and size-estimation logic into the
otherwise-pure projection layer.
Materializing the event avoids both.

### Auto reuses the `conversation.compaction.rules` array

Have automatic compaction apply the same rules manual compaction uses.
Rejected: those rules carry explicit, one-shot ranges that do not fit rolling
reapplication.
A standing rule with a rolling range is the right shape for an automatic
trigger; manual rules stay one-shot.

### Pre-turn evaluation

Evaluate the trigger before building the request, so the current turn is
protected from overflow.
Rejected for v1: it adds compaction, and possibly a summarization call, to the
latency of the turn itself.
Post-turn evaluation never touches the in-flight request and protects the next
turn.
A single turn that overflows on its own remains the domain of hard-fail and
truncation.

### Background task

Run compaction asynchronously, like title generation.
Rejected: compaction modifies the event stream, and concurrent modification
during a turn would need synchronization that does not exist today.
Between-turn compaction is simpler and safe.

### Tokenizer-based estimation

Use a model-specific tokenizer for an accurate count.
Deferred: it adds a dependency and per-model tokenizer selection, and a
conservative `trigger_ratio` over a character estimate is good enough for a
trigger decision.

## Non-Goals

- **Tiered or multi-rule automatic compaction.** A single rolling rule only.
  Layering multiple rolling tiers (for example, strip recent turns and summarize
  older ones, each on its own rolling boundary) is useful but needs composition
  semantics beyond this RFD.
  Fixed multi-range compaction is already served by manual `jp conversation
  compact` with a multi-rule config.
- **Automatic policy escalation.** If the configured rule does not bring the
  projection under the threshold, automatic compaction does not switch from
  mechanical to summarization, nor compact into the protected tail.
  Getting under stays a function of the rule and `keep_last`.
- **Token-accurate estimation**, as above.
- **Protecting the triggering turn from overflow.** Post-turn evaluation
  protects the next turn, not the current one.

## Risks and Open Questions

- **What `trigger_ratio` works in practice?** 0.75 is a starting guess.
  Conversations dominated by large tool responses may need a lower value.
  Needs experimentation.
- **What if the projection is still over the threshold after compacting?** The
  trigger fires once per boundary and then proceeds; it does not loop.
  This happens when the protected tail alone exceeds the budget, or when message
  text is too large for a mechanical rule (a summary rule is the fix).
  It is logged.
  Future tiered or escalating policies could address it.
- **Should a firing notify the user?** A subtle indicator during the next
  response could help but adds UI surface.
  The log and the `Auto` marker are sufficient initially.
- **Removing automatic compactions.** `auto.enabled = false` stops future
  firings but leaves materialized events in place (their summaries cost tokens
  to produce).
  `jp conversation compact --reset` removes all compaction events.
  A source-filtered reset (auto only) is a natural follow-up enabled by the
  marker.

## Implementation Plan

### Phase 1: Source marker

1. Add `CompactionSource` and a `source` field to `Compaction` (serde default
   `Manual`).
2. Set the source on the manual `compact` path and surface it in `jp
   conversation print`.

Backward compatible, no behavioral change.
Can merge independently.

### Phase 2: Configuration

1. Add `AutoCompactionConfig` (`enabled`, `trigger_ratio`, `min_turns`, `rule`)
   under `CompactionConfig`, with the schematic trait impls.
2. Default `rule` to the built-in mechanical policy.

No behavioral change until the trigger is wired.
Can merge independently.

### Phase 3: Trigger

1. Add the projected-size estimate, reusing the existing character heuristic.
2. Wire the trigger into the query turn loop, after turn completion and before
   persist: resolve the rolling range, build the `Compaction` (source `Auto`),
   generate the summary if configured, append, and log.
3. Skip silently when the context window is unknown.
4. Tests with a mock provider for the context window and the size thresholds.

Depends on Phases 1 and 2.

## References

- [RFD 064], Non-Destructive Conversation Compaction

[RFD 064]: ../064-non-destructive-conversation-compaction.md
