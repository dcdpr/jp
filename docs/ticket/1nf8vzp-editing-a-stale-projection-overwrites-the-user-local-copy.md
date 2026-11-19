# Editing a stale projection overwrites the user-local copy

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: client=cli
- **Label**: domain=conversation
- **Label**: domain=storage
- **Label**: package=jp_cli
- **Label**: package=jp_storage
- **Label**: type=bug

`jp conversation edit --events` (or `--metadata`, `--base-config`), run from a
checkout whose workspace projection is older than the user-local copy, opens the
older copy and then copies it over the newer one.
The turns made since are dropped from user-local storage without a diagnostic.

Found by reading the code during the review of RFD 138; not reproduced.
Confirm it with the test below before fixing.

## When it happens

A projected conversation is continued from checkout A, so the user-local copy
and A's projection hold the new turns.
Checkout B of the same workspace still holds an older projection of it, for
example a second worktree that checked out an earlier commit of
`.jp/conversations/`.

Every command that loads the conversation from B reads the user-local copy,
because its stream is the newer one (`Storage::stream_dir` in
`crates/jp_storage/src/load.rs`).
`jp conversation edit --events` from B does not.

## Why

- `resolve_paths` (`crates/jp_cli/src/cmd/conversation/path.rs`) resolves
  through `Storage::find_conversation_dir` (`crates/jp_storage/src/lib.rs`),
  which returns the workspace copy whenever one exists.
  RFD 031 asks for that preference in "Path and Editor Resolution"; the rule
  does not cover a projection that is older than the user-local copy.
- After the editor exits cleanly, `edit.rs` calls `sync_projection` for every
  edited conversation.
  `sync_projection` copies the workspace copy's managed files over the
  user-local ones (`sync_managed_files`) without comparing modification times.
- The sync runs whether or not the user changed anything, so opening the file
  and quitting is enough.
- `validate_edits` loads through the loader, which picks the newer copy.
  When the user saved nothing, it validates the user-local copy, not the file
  the sync then installs.

After the sync, user-local storage holds B's older stream with a fresh
modification time.
Checkout A then loads that copy instead of its own projection, and a later turn
run from A persists the older stream over A's projection too.

## Fix

To be decided between:

- Bring the workspace copy up to date before opening the editor when the
  user-local copy is the newer one, so the editor opens the stream JP loads.
- Refuse the edit, with a message naming both paths, when the workspace copy is
  the older one.

Either way, skip the sync for a conversation whose files the editor left
unchanged.
The pre-edit snapshots needed for that comparison are already taken.

## Verifying

A projected conversation whose user-local `events.json` holds one more turn than
the workspace copy and has the newer modification time.
Run a managed edit with an editor that exits without writing.
Assert the user-local copy still holds the extra turn.

A second case, where the editor changes the file, asserts the extra turn is not
dropped without a diagnostic.

## Related

- RFD 031: the path-preference rule and the resync after a managed edit.
- RFD 138: adds `jp conversation path --loaded` for readers of the same files,
  and leaves `edit` unchanged.
- T-0sqf1cs: the earlier `sync_projection` fix, which made it copy the managed
  files instead of replacing the directory.
