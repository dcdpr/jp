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
