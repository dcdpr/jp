# Let a conversation field track the workspace config

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-09-07

A conversation pins its configuration: workspace edits and JP upgrades leave it
resolving exactly as it did when it was created.
That is deliberate, and `--cfg WORKSPACE` is the explicit way to move a
conversation onto current workspace config.

What is missing is the same choice at field granularity.
There is no way to say "stop pinning `assistant.model.id` for this conversation,
follow whatever the workspace says from now on" while leaving every other field
pinned.

## Why the existing spellings do not cover it

`--cfg foo.bar=null` clears a field.
`null` is a value in the config language, so it means "no value" wherever it
appears, including in a config file that has no conversation to track.
The field resolves to the program default, not to the workspace.

`-C foo.bar=VALUE` ([RFD 070]) reverts to a previous claimed state.
That is a point in history, not a subscription: it answers "undo this
assignment", and the answer stops moving once it is given.

Both are one-time operations on a stored value.
Tracking is a standing instruction, which nothing in the model currently
expresses.

## Why it cannot be built yet

A conversation stores a resolved `AppConfig` and a list of deltas.
Nothing in it distinguishes "this value came from the workspace" from "I set
this deliberately", so there is no state to return a field *to*.

[RFD 070] introduces that distinction: `base` becomes the resolved workspace
configuration and `init` carries the creating invocation's contributions with
provenance.
Once a field's origin is recorded, "follow the workspace" has something to mean.

Even then, `base` is a snapshot taken at creation, so tracking needs a further
step: a field marked as tracking has to be resolved against the workspace files
at read time rather than against the stored base.

## Open questions

- How is it spelled?
  A keyword value (`foo.bar=inherit`) reuses the config language but
  reintroduces a value whose meaning depends on conversation history, which is
  what keeps `null` simple.
  A directive (`--track foo.bar`) keeps that separation at the cost of another
  flag.
- Does a tracked field survive a fork, or does the fork re-pin it?
- What does `jp conversation print` show for a turn whose tracked field has
  since changed in the workspace?
  The conversation records what its turns ran under, and a tracked field
  deliberately breaks that.
- Does tracking apply to a whole subtree (`--track style`), or leaves only?
- How does a tracked field interact with `-C`, which walks a history the tracked
  field is no longer part of?

## Related

- [RFD 070] Negative Config Deltas: introduces the `base` / `init` split this
  depends on, and the claim provenance that makes a field's origin knowable.
- [RFD 038] Config Reset Keywords: `WORKSPACE` is the whole-config version of
  this, and whatever spelling this lands on should read as its per-field
  sibling.

## Next step

Blocked on RFD 070 Phase 3.
Revisit once `base` and `init` exist, since the answer depends on what
provenance is actually recorded.

[RFD 038]: ../rfd/038-config-reset-keywords.md
[RFD 070]: ../rfd/070-negative-config-deltas.md
