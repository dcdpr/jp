# Give partial collections a state for "no opinion" distinct from "empty"

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-09-08

`MergeableVec::Vec([])` and `MergeableMap::Map({})` mean both "this layer says
nothing about the field" and "this field is empty".
`delta_mergeable_vec` and `delta_mergeable_map` are entitled to either reading,
and pick opposite ones depending on what they are handed.

A plain `Option<Vec<T>>` field has no such problem: `None` is "no change" and
`Some(vec![])` is "empty", and the two never collide.
The strategy-carrying collections lost that distinction when they gained their
merge metadata.

## Why now rather than earlier

T-0ffsw7e removed the only caller that read a delta as though it were a
snapshot, so nothing currently reaches the ambiguity.
It is unreachable, not absent.

#1130 and #1131 move every list and map in the config behind these two helpers,
which turns a property of two access rule lists into a property of the whole
config tree.
The next caller that diffs a value of uncertain provenance rediscovers it,
across a much wider surface.

## Shape

Either an `Option` around the collection in the partial, or an explicit "no
opinion" variant on the wrappers themselves.

The cost is a serialization shape change on every collection config field, plus
every `delta`, `fill`, `merge`, and `AssignKeyValue` impl that touches one.
That is why it is its own ticket and not part of the fix.

## Independent motivation

On the resolved side an empty `access.fs` means "unrestricted, workspace-
confined".
So `[]` is a meaningful value there, and the partial needs a way to say "no
opinion" that is not the same bytes.

## Comments

-----

- **From**: jp
- **Date**: 2026-09-08T19:05:48Z

Scope note, from review of #1135.

This ticket is closed as invalid on the `ticket-0fgdq7z` branch, on the finding
that for every field involved the empty collection is the identity element of
the merge that field uses, so "absent" and "empty" behave identically and the
distinction buys nothing.

That finding covers **empty-versus-absent for collections** and nothing else.
It does not extend to the rest of what a partial carries and a resolved config
does not.

`MergedString` and `MergedVec` also hold `strategy`, `separator`,
`discard_when_merged`, and `dedup`.
Of those, `dedup` and `discard_when_merged` are read from the *accumulated* side
of a merge (`internal/merge/string.rs:23,26`), and `dedup` is documented as
sticky (`types/string.rs:225-227`): once a layer states one, later merges use
it.
So erasing it is not inert, and the argument that closed this ticket says
nothing about it.

Two facts bound how far it reaches today:

- `partial_via = MergeableString` appears once, on `assistant.system_prompt`.
  That is the only field whose metadata is writable from `--cfg`, through the
  nested-key path at `assistant.rs:105-110`.
- `MergeableVec` and `MergeableMap` have no `AssignKeyValue` impl at all, so
  their metadata cannot be written from the command line in either direction.
  `types/vec.rs:32-37` contemplates adding one, and the day that lands the
  surface widens to every collection field.

#1135 fixes the half of this that corrupts: an override is merged onto the
conversation's accumulated partial rather than onto a resolved-and-re-derived
one, so a recorded `dedup` governs the comparison.
The half that remains is an override changing *only* metadata — it leaves every
resolved value alone, so it compares equal and is dropped.
That is recorded in `override_to_record`'s doc comment rather than as a ticket,
because the fix is not obvious: reading the metadata means comparing two
partials, and merging is not shape-preserving.
Whether it collapses a field stamped `replace` depends on which sibling fields
the override touches, so equal metadata can sit in unequal shapes and partial
equality is not a usable instrument.

Written down so the next reader does not take "empty and absent behave
identically" for the wider claim that nothing is lost in resolution.
Something is; it is just not what this ticket described.
