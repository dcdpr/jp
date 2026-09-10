# A cleared conversation field is refilled from the file layer on the next invocation

- **Status**: Done
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-10
- **Label**: client=cli
- **Label**: domain=conversation
- **Label**: package=jp_cli
- **Label**: package=jp_config
- **Label**: package=jp_conversation
- **Label**: type=bug

`jp query --cfg assistant.name=null` clears the field for that turn.
The next `jp query` on the same conversation runs with the workspace's value
again, and writes a config delta restoring it, so the clear is undone
permanently rather than merely forgotten.

Any field a config file also declares behaves this way.
A field only the conversation ever set stays clear, which is why the delta-level
tests pass.

## Why

The stream is correct.
A delta carries the cleared path in `unsets`, `fold_config_delta` applies it
before merging, and `ConversationStream::config` resolves the field to `None`.

The invocation pipeline then puts the value back.
`Query::apply_conversation_config` (`crates/jp_cli/src/cmd/query.rs:2392`) turns
the stream's resolved config into a partial, where the cleared field is `None`.
`ConfigPipeline::partial_with_conversation`
(`crates/jp_cli/src/config_pipeline.rs:322`) then calls
`conversation.fill_from(self.base.clone())`, and filling reads `None` as "this
layer says nothing".
`PartialAssistantConfig::fill_from` (`crates/jp_config/src/assistant.rs:166`) is
`self.name.or(defaults.name)`, so the file layer's value wins.

Nothing in `config_pipeline.rs` mentions `unset`: the cleared paths are consumed
by the fold and never cross into the pipeline.

The reversal then persists.
`get_config_delta_from_cli` diffs the stream (`None`) against the runtime config
(`Some("Bot")`), `delta_opt_at` takes its `(None, next) => next` arm, and a
delta setting the field back is recorded.

## Scope

Not a regression.
Before deltas could express a clear, the clear was never recorded and the
outcome was the same value on the next turn.
What changed is where it is lost: the stream now holds the clear, and the layer
above it overrides.

The fill semantics themselves are right, and are what stop an appending list
from doubling when the conversation snapshot merges over the layer it came from.
The gap is that a partial cannot distinguish "cleared" from "not stated", which
is the same distinction `unsets` exists to carry.

## Fix

Carry the conversation's cleared paths across the boundary.

`ConversationStream` grows an accumulator over its deltas' `unsets` — a path
cleared by one delta and set by a later one is no longer cleared — and
`partial_with_conversation` applies what it reports after `fill_from`, before
`--cfg` merges on top.
A `--cfg` that sets the field in the same invocation still wins, which is what a
user typing it expects.

## Verifying

A two-invocation regression through the pipeline, which is the boundary the
`jp_config` sweep in `delta_law_tests.rs` cannot reach: a workspace config
naming `assistant.name`, one query with `--cfg assistant.name:=null`, then a
second with no flag.
Assert the resolved name is still clear, and that the second query wrote no
delta restoring it.

## Comments

-----

- **From**: jp
- **Date**: 2026-09-10T18:10:17Z

Implemented `ConversationStream::config_unsets()` to report cleared paths that
remain absent after folding the stream, excluding clears before a reset and
paths with replacement values.
The conversation-config hook carries those paths alongside its partial;
`ConfigPipeline` reapplies them after base-layer filling and before `--cfg`
directives.

The regression calls the production `resolve_config` and `turn_config_delta`
paths for successive invocations.
It failed before the fix with `Some("Bot")` on the second invocation.
It passes with the name still `None` and no second config delta.
Additional tests cover later sets, same-delta replacements, resets, removed map
entries, duplicate/unknown paths, and CLI override precedence.

Validation: full `jp_cli` and `jp_conversation` test suites pass; scoped Clippy
checks pass without warnings; formatting and the diff were reviewed.
