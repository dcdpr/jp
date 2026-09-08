# A recomputed config delta clears an unchanged access rule list

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-08

On a continuing conversation, a `--cfg` that changes `access.env` for a tool
also clears that tool's `access.fs` rules.
An empty `fs` list means unrestricted (workspace-confined) access, so the tool
silently gains filesystem reach the user never granted, and the widening
persists for every later turn.

The same happens in reverse, and for any pair of sibling rule lists: whichever
one did *not* change is the one that gets cleared.

## Why

`get_config_delta_from_cli` (`crates/jp_cli/src/cmd/query.rs:1860`) diffs the
conversation's config against the invocation's.
For a rule list that did not change, `rule_delta`
(`crates/jp_config/src/conversation/tool/access.rs:141`) returns an empty append
delta — `MergeableVec::Vec([])`, meaning "nothing to add".

`PartialAccessConfig` does not override `delta_with_unsets`, so it takes the
default at `crates/jp_config/src/delta.rs:37` and reports no cleared paths.
With `unsets` empty, `ConversationStream::add_config_delta`
(`crates/jp_conversation/src/stream.rs:587`) takes its re-diff branch and calls
`config.to_partial().delta(*delta)` — diffing the *delta* as though it were a
snapshot.

On that second pass the empty list is no longer "nothing to add"; it reads as a
complete rule set of zero rules.
`[].starts_with(&[rule])` is false, so `rule_delta` falls through to
`Replace([])`, and the fold resolves the field to empty.

An empty `MergeableVec` cannot distinguish "no change" from "cleared", so the
two passes are entitled to opposite readings of the same value.
No tweak to `rule_delta` alone removes the ambiguity.

## Scope

Not introduced by the rule-ordering fix in #1126.
Substituting the previous `rule_delta` body, an unchanged list also returned
`Vec([])` (`prev.iter().all(|r| next.contains(r))` holds), and the second pass
also emitted `Replace([])`.

The trigger is wider than a reorder: any delta carrying an access block whose
sibling list is unchanged, including a plain `--cfg` append of one env rule.

Only fields that never report an unset are exposed.
A delta carrying at least one cleared path is stored verbatim, so plain lists
going through `delta_opt_vec_at` are already immune.
Access rule lists are reachable precisely because they report nothing.

## Fixes

Two candidates, both defensible.

The root fix is to stop re-diffing a delta that was already computed against the
right state.
`add_config_delta` has the shape of the answer — it stores a delta as given
when `unsets` is non-empty, for exactly this reason — but it infers "already
computed" from a side effect instead of being told.
Carrying that explicitly on `ApplyDelta` fixes the whole class, including field
types added later.

The narrower fix is to give `PartialAccessConfig` a `delta_with_unsets` that
reports the field's path whenever `rule_delta` falls back to `Replace`, threaded
down through the tool and conversation partials the way the other reporting
types already do.
That is the `unsets` mechanism used as designed: a field emitting `Replace` is
saying merging cannot reach the value.
It leaves the second pass in place for any future field that states a strategy
without reporting one.

The root fix is preferable: the map and list conversions in flight put more
strategy-carrying fields on this path.

## Verifying

The delta law suite in `crates/jp_config/src/delta_law_tests.rs` cannot catch
this — it runs one delta-and-merge cycle.
A regression needs the query's calculation and `add_config_delta` together: a
conversation holding `fs` rules, an invocation changing only `env`, and an
assertion that the resolved `fs` rules survive.
