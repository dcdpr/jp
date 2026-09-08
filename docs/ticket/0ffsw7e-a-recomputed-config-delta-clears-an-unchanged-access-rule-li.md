# A recomputed config delta clears an unchanged access rule list

- **Status**: Done
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

## Comments

-----

- **From**: jp
- **Date**: 2026-09-08T10:01:44Z

Fixed by making `ConversationStream::add_config_delta` record what it is given.

## Corrections to the report

**The report describes #1126's branch, not `main`.** `delta_law_tests.rs` and
the `starts_with` body of `rule_delta` only exist there.
The Scope section's both-versions check holds, so the conclusion stands either
way.

**A worse case sits next to the reported one.** Adding a rule drops the rules
already there, which needs no sibling list at all.
A conversation granting `access.fs = [src]` and an invocation granting `[src,
docs]` produce the append-shaped delta `Vec([docs])`; re-read as a complete rule
set that says the tool has exactly one rule, and the fold resolves `fs` to
`["docs"]`.
Confirmed by `appending_an_access_rule_keeps_the_rules_already_there`, which
fails with `["docs"]` against the old body.

**`conversation.labels` was exposed too**, through `labels_delta`: an unchanged
map re-diffs to every key dropped, so the whole map is replaced with an empty
one.
`conversation.attachments` was not — `attachments_delta` has no replace
fallback.

**Four callers passed a raw partial, not a delta** (`config/set.rs:58`,
`summarize.rs:67`, `inquiry.rs:286`, `query.rs:611`), so for them the *first*
pass was already the category error.
`jp config set` on a conversation with label rules cleared them with no `--cfg`
naming labels.

**`ConversationStream::extend` double-diffed as well** (`stream.rs:1608`),
corrupting stream reconstruction — which is how a summary request is built.
Confirmed by `extending_a_stream_reproduces_the_config_of_each_event`.

**Plain lists were immune for a different reason than stated.** Not because a
delta carrying unsets is stored verbatim, but because `Option<Vec<T>>` has a
`None` that means "no change" and is distinct from `Some(vec![])`.
That is the invariant the fix restores.

## What landed

Neither proposed candidate.
The narrow fix cannot work: for the four raw-partial callers,
`delta_with_unsets` is never on the path.
The root fix turned out not to need a flag on `ApplyDelta` either — once
nothing re-diffs, a sparse partial *is* a valid delta ("merge these values"), so
the two inputs stop needing to be told apart.
`add_config_delta` now drops an apply that carries nothing and appends
everything else.

`add_config_reset` no longer duplicates the append: it existed separately only
because `add_config_delta` resolved the stream config, and
`test_add_config_reset_appends_reset_then_nonempty_layers` fails against the old
body, which proves the reason is gone.

Two effects beyond the bug: the stored JSON gets *smaller* (the re-diff wrapped
lists in a `{value, strategy, discard_when_merged}` envelope that is now just
the list), and `extend` loses a full config resolution per event, which was
O(n²) folds over a stream.

## Behaviour change to be aware of

`jp config set` with a value the conversation already resolves to now records
the delta instead of dropping it, pinning it against later workspace changes.
Pinned by `set_in_conversation_records_a_value_already_in_effect`.

## Coverage

Six tests, each watched failing against the old body before being kept:
`appending_an_access_rule_keeps_the_rules_already_there`,
`changing_env_rules_keeps_the_fs_rules`,
`extending_a_stream_reproduces_the_config_of_each_event`,
`a_stored_delta_holds_only_the_fields_that_changed` (exact JSON),
`without_an_unset_a_dropped_argument_appends_instead`, and
`set_in_conversation_records_a_value_already_in_effect`.

`Config Delta` is now in `docs/architecture/ubiquitous-language.md`, with the
delta-versus-snapshot distinction this bug came from.

## Still open

The ambiguity itself survives: `delta_mergeable_vec` and `delta_mergeable_map`
still cannot tell "nothing to add" from "cleared".
Nothing re-diffs a delta any more, so it is unreachable rather than absent.
#1130 and #1131 put every list and map in the config behind those helpers, so
the follow-up — giving the partial collections a state for "no opinion"
distinct from "empty" — is worth a ticket of its own.

-----

- **From**: jp
- **Date**: 2026-09-08T10:11:36Z
- **Re**: #1

One more consequence, raised in review and worth recording: the old body never
converged.

A conversation granting `access.fs = [src]` against an invocation granting
`[src, docs]` stored `Replace([docs])`, so the conversation resolved to
`["docs"]`.
The next identical run then saw `[docs]` against the wanted `[src, docs]` and
produced `Vec([src])`, which re-diffed to `Replace([src])` and dropped `docs`.
The run after that swapped them back.

So every `jp` run on such a conversation appended a config delta and moved the
resolved rule set, indefinitely.
The `{value, strategy, replace}` envelope in the stored delta was what caused
this, not what prevented it.

`a_repeated_invocation_records_no_second_delta` covers it: the second
invocation's diff must be empty and the stream must still hold one delta.
It fails against the old body with a delta re-adding the dropped rule.

Also recorded: `set_in_conversation_twice_records_two_deltas`.
`jp config set` passes values rather than a diff, so a repeat writes a second
delta where the old body dropped it.
Bounded by explicit invocations, each event the size of the `--cfg` values.

A value-equality check in `set.rs` was considered and rejected: comparing a
sparse partial field-wise against a resolved snapshot is the operation whose
ambiguity caused this bug.
The version that would be correct asks whether the conversation already pins the
value, which needs a fold over its own deltas alone.

-----

- **From**: jp
- **Date**: 2026-09-08T10:23:58Z
- **Re**: #2

Regression fixed in the same branch: a `config set` naming a value already in
effect no longer records an event.

The check is not a field-wise comparison of the override against the resolved
config — that is the sparse-versus-snapshot comparison this whole ticket is
about, and it carries the same ambiguity for every collection field.

`config_pipeline::override_to_record` merges the override onto the current
config, resolves both sides, and compares the resolved values:

```rust
let base = current.to_partial();
let mut merged = base.clone();
merged.merge(&(), overrides.clone())?;

let (Ok(before), Ok(after)) = (build(base), build(merged)) else {
    return Ok(Some(overrides));
};

Ok((before != after).then_some(overrides))
```

Resolved configs rather than partials, because a partial also carries the merge
strategy each field arrived with: two partials holding identical values can
still compare unequal, and resolution normalizes that away.
Both sides go through the same transform, so the comparison does not rest on
`to_partial` round-tripping exactly.
An override whose merged result does not resolve is recorded rather than dropped
— its outcome cannot be compared, so it is not known to be a no-op.

The function names no field and knows no merge strategy; every per-field
decision stays with the `#[setting(merge = ...)]` on the field.

Wired into both persisted override call sites: `config/set.rs` and the
editor-provided config in `query.rs`.
The two throwaway-stream callers (`summarize.rs`, `inquiry.rs`) are left alone;
nothing they append is persisted.

Five tests on the helper, covering how a field merges against whether the
override changes it: replacement restated and changed, a strategy-carrying list
restated and extended, and an appending list restated.
That last one records, which is correct — appending without deduplicating is
not idempotent, so asking for the same element twice is a real change. #1130
gives most lists dedup-by-default and adds `ordered_vec_with_strategy` for the
ones where repetition is meaningful, which splits that case deliberately.

`set_in_conversation_skips_a_value_already_in_effect` and
`set_in_conversation_twice_records_one_delta` cover it end to end.
Both were watched failing first.
