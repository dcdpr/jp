# `Workspace::open` returns a workspace with an empty conversation index

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-08-19

`Workspace::open` returns a value whose `conversations()` yields nothing,
however many conversations the store holds, until the caller separately
remembers `load_conversation_index()`.
Nothing in the type says so, and the failure is silent: an empty iterator is
indistinguishable from an empty workspace.

Every consumer today gets it right — `jp_cli` at `lib.rs:500`, `jp_ffi` in
`jp_workspace_open` — so this is a hazard rather than a live bug.
It is the kind that only surfaces at the third consumer.

## Why `open` cannot simply load it

The obvious fix is wrong.
`sanitize.rs:15` requires sanitization to run first: "Call this before
`load_conversation_index` to guarantee the backing store is consistent."
`jp_cli` follows that order — the trashed-conversation sweep at
`lib.rs:480-493`, then the index at line 500.
Indexing inside `open` would index a store that has not been repaired.

## Two shapes that work

**A preparation operation that owns both steps.** `open` stays as it is, and a
single call runs sanitize-then-index in the required order, so a caller cannot
get it half right.
Cheapest, and leaves the current constructors alone.

**Encode the unloaded state.** `open` returns a value that has no
conversation-reading methods, and `load_conversation_index` consumes it and
returns one that does.
Makes the mistake unrepresentable rather than merely documented, at the cost of
a second type and a change to every construction site.

Both touch every construction site in `jp_workspace` and `jp_cli`, which is why
neither rode along with the move of workspace opening into `jp_workspace`.

## Interim

`open`'s doc comment states the contract: the index is not populated,
`load_conversation_index` is the caller's next step, and `sanitize` comes before
it.
That paragraph can go once the ordering is enforced rather than described.
