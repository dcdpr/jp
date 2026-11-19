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

Running `jp conversation edit --events` (or `--metadata`, `--base-config`) from
a checkout whose workspace projection is older than the user-local copy opens
the stale workspace projection and then overwrites the newer user-local copy
with it.
Any turns added since the projection was last updated are silently lost from
user-local storage.

Identified by code inspection while reviewing RFD 138; reproduce with the test
case below before fixing.

## When it happens

Suppose a projected conversation is continued from checkout A, updating both
user-local storage and checkout A's projection.
Checkout B of the same workspace still has an older projection (for example, a
secondary worktree checked out at an earlier commit of `.jp/conversations/`).

Every command that loads the conversation from checkout B reads the user-local
copy because its stream has the newer modification time (`Storage::stream_dir`
in `crates/jp_storage/src/load.rs`).
However, running `jp conversation edit --events` from checkout B opens and syncs
checkout B's stale projection.

## Why

- `resolve_paths` (`crates/jp_cli/src/cmd/conversation/path.rs`) calls
  `Storage::find_conversation_dir` (`crates/jp_storage/src/lib.rs`), which
  always returns the workspace projection path when one exists.
  RFD 031 specifies this workspace preference in "Path and Editor Resolution"
  without accounting for a projection that is older than the user-local copy.
- When the editor exits with status 0, `edit.rs` calls `sync_projection` for
  each edited conversation.
  `sync_projection` copies the workspace projection's managed files over the
  user-local files (`sync_managed_files`) without comparing modification times.
- The sync runs even if the user made no edits in the editor; opening the file
  and immediately quitting is sufficient to overwrite the user-local copy.
- `validate_edits` loads the conversation through the normal loader, which
  selects the newer copy.
  When the user exits without saving, `validate_edits` validates the newer
  user-local copy right before `sync_projection` overwrites it with the stale
  workspace projection.

After `sync_projection` finishes, the user-local copy holds checkout B's stale
stream with a fresh modification time.
Checkout A then loads that overwritten user-local copy instead of its own newer
projection, and the next turn in checkout A overwrites checkout A's projection
as well.

## Fix

Choose one of two behaviors when the workspace projection is older than the
user-local copy:

- Update the workspace projection from the user-local copy before opening the
  editor so the user edits the stream JP actually loads.
- Refuse the edit with an error naming both paths.

In addition, skip `sync_projection` whenever the editor leaves the target files
unchanged (using the pre-edit snapshots that `edit.rs` already captures).

## Verifying

Create a projected conversation whose user-local `events.json` contains one more
turn and a newer `mtime` than the workspace projection:

- Run `jp conversation edit --events` with a no-op editor that exits without
  writing, and assert that the user-local copy retains the extra turn.
- Run `jp conversation edit --events` with an editor that modifies the file, and
  assert that the newer user-local turn is not silently overwritten.

## Related

- RFD 031: Workspace path preference and post-edit projection resync.
- RFD 138: Adds `jp conversation path --loaded` for read-only consumers while
  leaving `jp conversation edit` unchanged.
- T-0sqf1cs: Earlier `sync_projection` fix that switched from replacing the
  entire directory to copying only the three managed files.
