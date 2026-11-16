# RFD D50: Automatic Compaction

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-15
- **Extends**: [RFD 064]
- **Summary**: Appends a rolling compaction overlay automatically between turns
  when a conversation's projected size exceeds a configured context-window
  ratio.

## Summary

This RFD extends [RFD 064] with automatic compaction: when a conversation's
projected size exceeds a configured ratio of the model's context window, JP
appends a rolling compaction event between turns.
Automatic compaction reuses the existing compaction event model and projection
layer, adding only a trigger and a standing rule.
It is disabled by default.

## Motivation

[RFD 064] implements compaction as a manual command: users must notice when a
conversation is growing long and run `jp conversation compact`.
In long coding sessions, the context window fills gradually, so response quality
degrades and token costs rise before the user intervenes.
Keeping the working context small should not require manual monitoring.

[RFD 064] reserved the `conversation.compaction.auto` namespace for automatic
compaction; this RFD defines it.

## Design

### Configuration

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

`auto.rule` is a single `CompactionRuleConfig`, using the same schema as entries
in `conversation.compaction.rules` (multi-rule automatic compaction is a
non-goal).
It defaults to the built-in mechanical policy, which strips reasoning and tool
calls while keeping the first turn and the last three turns.
Adding a `summary` block switches the rule to summarization, matching manual
rules.

### Materialized compaction events

When the trigger fires, automatic compaction appends a `Compaction` event to the
stream, using the same event type that `jp conversation compact` produces.
Automatic compaction is a new trigger for the existing compaction mechanism; the
projection layer does not change, and it never infers compaction at read time
from the configuration and turn count:

1. **Eager summaries**: [RFD 064] stores generated summary text directly in the
   `Compaction` event because generating summaries is costly and
   non-deterministic.
   Inferring compaction at projection time would regenerate summaries on every
   read (`jp query`, `jp conversation print --compacted`).
2. **Pure projection layer**: `Thread::into_parts()` is a pure function of the
   event stream.
   Evaluating the trigger requires the model's context window and a token-count
   estimate, which are runtime inputs available in the turn loop.
   Materializing the `Compaction` event in the turn loop keeps runtime model
   details out of the projection layer.

### Compaction source marker

The `Compaction` type gains a source marker:

```rust
pub enum CompactionSource {
    Manual,
    Auto,
}
```

Existing events deserialize as `Manual` (the serde default) for backward
compatibility.
The marker lets `jp conversation print` indicate which overlays were applied
automatically and supports a future selective reset, without affecting
projection.

### Size estimation and threshold

After a turn completes and before the stream is persisted, JP estimates the
conversation's projected token count and compares it against the trigger
threshold:

```
estimated_tokens = projected_character_count / 4
threshold        = context_window * trigger_ratio
```

The estimate is computed over the *projected* view (with existing compactions
applied) rather than the raw event stream.
Once a compaction runs, the projected size drops below the threshold until
future turns grow the context again.
Estimating the raw stream would re-trigger on every subsequent turn because
overlay events never shrink the raw history.

`context_window` comes from `ModelDetails::context_window` on the active
provider, which is an `Option<u32>`.
When the context window is unknown (for example, a local model without
metadata), JP logs at debug level and skips automatic compaction rather than
failing.

### Rolling range

The appended compaction covers:

```
from = max(keep_first, after_last_compaction)
to   = last_turn - keep_last
```

`keep_first` sets a permanent floor so the opening turns are never compacted.
`after_last_compaction` is the turn following the furthest-compacted turn in the
stream (the existing `RangeBound::AfterLastCompaction`, already used by `--from
last`).
Taking the maximum of the two ensures that each firing compacts only turns that
no earlier compaction has covered.
`keep_last` preserves the most recent turns at the tail of the conversation.

Concretely, with `keep_first = 1` and `keep_last = 3`:

- After turn 12, with no prior compaction, JP compacts turns `[1, 9]`.
  `keep_first` preserves turn 0, and `keep_last` preserves turns 10 through 12.
- After turn 20, with the first compaction ending at turn 9, JP compacts turns
  `[10, 17]`.

Because compacted bands never overlap, each summary covers its band once,
generated from the raw events, and automatic compaction never triggers [RFD
064]'s summary-overlap auto-extension.
The projected view accumulates sequential summary blocks followed by the live
tail.

### Turn-boundary execution

At the end of each turn, before persisting the stream, JP evaluates automatic
compaction at most once (it never loops):

1. Check that `auto.enabled` is `true`, the conversation has more than
   `min_turns` turns, `ModelDetails::context_window` is known, and
   `estimated_tokens > threshold`.
   If any check fails, stop.
2. Resolve the rolling range `[from, to]`.
   If the range is empty, stop.
3. Build a `Compaction` event (source `Auto`) from `auto.rule`.
   If the rule configures a summary, generate it from the raw events in `[from,
   to]`.
4. Append the event to the stream, persist the conversation, and log the
   compacted range, policy, and estimated reduction.

### Prompt cache interaction

Appending a compaction changes the projected message prefix, so the turn
immediately following an automatic compaction incurs a one-time prompt-cache
miss (the system-prompt cache uses a separate prefix and is unaffected).
That single cache miss trades one uncached request for a smaller context on
subsequent turns.
While [RFD 064] kept compaction manual to avoid unexpected cache invalidation,
the 0.75 default `trigger_ratio` and the once-per-turn cap keep firings
infrequent.

## Drawbacks

- **Less visible context reduction**: Users may not notice when automatic
  compaction reduces the projected context.
  To mitigate this, automatic compaction is disabled by default, logs each
  firing, marks events as `Auto` for `jp conversation print`, and preserves the
  raw events in the stream.
- **Approximate token estimation**: Character-based estimation can be off by a
  factor of two or three depending on the content.
  `trigger_ratio` provides a safety margin, and a tokenizer-based estimate can
  replace the heuristic later without changing the event model.
- **Event accumulation**: Long sessions accumulate one `Auto` compaction event
  per firing.
  These events are small metadata records over non-overlapping turn bands and
  can be cleared with `--reset`; coalescing adjacent mechanical bands is a
  possible future optimization.
- **Prompt-cache invalidation**: Each firing modifies the projected message
  prefix and causes a one-time prompt-cache miss on the next turn.

## Alternatives

### Config-inferred projection (no event)

Computing the compacted view at read time from the `auto` config and turn count
without storing an event would require regenerating eager summaries on every
read and would push context-window and size-estimation logic into the pure
projection layer.
Materializing a `Compaction` event avoids both problems.

### Reusing `conversation.compaction.rules`

Applying `conversation.compaction.rules` directly during automatic compaction
does not fit rolling execution because those rules carry explicit, one-shot
ranges.
Automatic compaction instead uses a single standing rule (`auto.rule`) with a
rolling range, while manual rules remain one-shot.

### Pre-turn evaluation

Evaluating the trigger before building the provider request would protect the
current turn from context overflow, but it would add compaction (and potentially
a summarization call) to the latency of the turn itself.
Post-turn evaluation leaves the in-flight request untouched and protects the
next turn.
A single turn that overflows the context window on its own remains handled by
hard-fail and truncation.

### Background task

Running compaction asynchronously like title generation would modify the event
stream concurrently with an active turn, requiring synchronization that does not
exist today.
Running compaction synchronously between turns is simpler and avoids concurrent
stream mutation.

### Tokenizer-based estimation

Using a model-specific tokenizer for exact token counts is deferred.
It adds a dependency and per-model tokenizer selection, whereas a conservative
`trigger_ratio` over the character heuristic is sufficient for triggering
compaction.

## Non-Goals

- **Tiered or multi-rule automatic compaction**: Automatic compaction supports a
  single rolling rule.
  Layering multiple rolling tiers (for example, stripping recent turns and
  summarizing older ones across separate rolling boundaries) requires
  composition semantics beyond this RFD.
  Fixed multi-range compaction is already supported by manual `jp conversation
  compact` with a multi-rule config.
- **Automatic policy escalation**: If the configured rule does not bring the
  projected size below the threshold, automatic compaction does not escalate
  from mechanical stripping to summarization or compact into the protected tail.
  Bringing the projection below the threshold remains a function of `auto.rule`
  and `keep_last`.
- **Token-accurate estimation**: Automatic compaction uses the character-count
  heuristic rather than a model-specific tokenizer.
- **Protecting the triggering turn from overflow**: Post-turn evaluation reduces
  context size for the next turn rather than the current one.

## Risks and Open Questions

- **What `trigger_ratio` works in practice?**: `0.75` is an initial estimate.
  Conversations dominated by large tool responses may need a lower threshold,
  which requires experimentation.
- **What if the projection remains over the threshold after compacting?**: The
  trigger fires at most once per turn boundary and does not loop.
  The projected size can stay above the threshold when the protected `keep_last`
  tail exceeds the budget on its own, or when message text is too large for a
  mechanical rule (which a summary rule resolves).
  JP logs this condition when it occurs, and future tiered or escalating
  policies could address it.
- **Should a firing notify the user?**: Showing a subtle indicator during the
  next response could help, but it adds UI surface.
  Logging the firing and recording the `Auto` source marker are sufficient
  initially.
- **Removing automatic compactions**: Setting `auto.enabled = false` stops
  future firings but leaves materialized events in place because their summaries
  cost tokens to generate.
  `jp conversation compact --reset` removes all compaction events, and the
  `CompactionSource` marker enables a future source-filtered reset for `Auto`
  events only.

## Implementation Plan

### Phase 1: Source marker

1. Add `CompactionSource` and a `source` field to `Compaction` (serde default
   `Manual`).
2. Set the source on the manual `compact` path and surface it in `jp
   conversation print`.

This phase is backward compatible, introduces no behavioral change, and can
merge independently.

### Phase 2: Configuration

1. Add `AutoCompactionConfig` (`enabled`, `trigger_ratio`, `min_turns`, `rule`)
   under `CompactionConfig`, with the schematic trait impls.
2. Default `rule` to the built-in mechanical policy.

This phase introduces no behavioral change until the trigger is wired and can
merge independently.

### Phase 3: Trigger

1. Add the projected-size estimate, reusing the existing character heuristic.
2. Wire the trigger into the query turn loop, after turn completion and before
   persist: resolve the rolling range, build the `Compaction` (source `Auto`),
   generate the summary if configured, append, and log.
3. Skip automatic compaction (logging at debug level) when the context window is
   unknown.
4. Add tests with a mock provider covering context-window discovery and size
   thresholds.

This phase depends on Phases 1 and 2.

## References

- [RFD 064], Non-Destructive Conversation Compaction

[RFD 064]: ../064-non-destructive-conversation-compaction.md
