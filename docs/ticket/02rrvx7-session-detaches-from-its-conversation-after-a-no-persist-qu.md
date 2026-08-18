# Session detaches from its conversation after a `--no-persist` query

- **Status**: Done
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-15

Running `jp -! q --new ...` in a terminal loses that terminal's active
conversation.
The next `jp q` in the same tab opens the conversation picker instead of
continuing where it left off.

Reproduced by `just rfd-summaries`, which runs `jp -! q --format=json --new`
once per changed RFD.

## Cause

`--no-persist` swaps the persist and lock backends to their Null variants but
leaves the session backend pointing at real storage.
`Query::run` records the session activation unconditionally, so the ephemeral
conversation — which is never written to disk — lands at the front of the
session's history.

`Workspace::session_active_conversation` reads `history[0]` and filters it
against the live conversation index.
The entry misses, so it returns `None` and the caller falls through to the
picker, even though the real conversation is still sitting at `history[1]`.

## Related

Two nearby defects in `cleanup_stale_files`, both reachable from a
`--no-persist` run whose lock backend reports nothing locked:

- Lock state is read from the workspace's lock backend, so an ephemeral run can
  delete another process's session mapping while its conversation is mid-write.
- Archived conversations are absent from the live index, so archiving one prunes
  it from every session's history.
  Unarchiving restores a conversation the session has forgotten.

## Comments

-----

- **From**: jp
- **Date**: 2026-08-15T06:17:28Z

Fixed in `b90fd2c4`.

Three changes:

- **`ReadOnlySessionBackend`** (`jp_storage::backend::null`) wraps the real
  session backend: reads pass through, writes are dropped.
  `load_workspace` installs it alongside the existing Null persist and lock
  swaps, so an ephemeral run can resolve the session's conversation but can't
  record one.

- **`Workspace::resolvable_history`** backs `session_active_conversation` and
  `session_previous_conversation`.
  It walks the history and returns the entries that still resolve, skipping
  conversations present in neither the live index nor the archive.
  The walk stops at an archived entry, so archiving the active conversation
  leaves the session without a target and the caller opens the picker rather
  than silently moving the user to an older conversation.

- **`cleanup_stale_files`** reads lock state from the `FsStorageBackend` it is
  already handed instead of the workspace's lock backend, and counts the archive
  partition as live when pruning history entries.

Tests: five in `session_mapping_tests.rs` (ghost skipped for active and
previous, archived head stops the walk, cleanup keeps archived and mid-write
entries), three in `null_tests.rs` for the read-through/write-discard backend.
Each of the five workspace tests was run against the unfixed code and observed
red first.

Not covered: an end-to-end test that `jp -! q` leaves the session file
byte-identical.
That path needs a live provider, so coverage stops at the storage and workspace
layers plus the wiring in `load_workspace`.
