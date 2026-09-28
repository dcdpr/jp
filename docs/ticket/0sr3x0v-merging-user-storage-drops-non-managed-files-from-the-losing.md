# Merging user storage drops non-managed files from the losing conversation copy

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-27
- **Label**: domain=storage
- **Label**: package=jp_storage
- **Label**: type=bug

When the same conversation exists on both sides of a user-storage merge, the
copy that loses the mtime comparison is deleted whole, so a query draft
(`QUERY_MESSAGE.md`) or any other file kept only in that copy is lost without a
diagnostic.

## Why

`adopt_conversation_dir` (`crates/jp_storage/src/lib.rs`) keeps whichever copy
has the newer `dir_mtime`:

- When the source is newer, it runs `fs::remove_dir_all` on the destination and
  moves or copies the source into its place.
  The destination's non-managed files go with it.
- When the destination is newer, it leaves the source alone.
  But for a sibling merge (`merge_sibling_user_workspace_dirs`), the whole
  sibling directory is removed afterwards, taking the source's non-managed files
  with it.

Both paths run from `migrate_user_storage`: the sibling merge on every run where
legacy per-worktree directories exist, and the workspace import on a workspace's
first run.

## Fix

Pick the winner's managed files, not the winner's directory.
Bring the destination to the winning name, then carry across the files the loser
holds that the winner does not, the way `sync_projection` copies only
`MANAGED_FILES` and leaves everything else alone.
When both copies hold the same non-managed file with different content, keep the
newer one and log a warning rather than delete silently.

## Verifying

A sibling merge where the losing copy holds `QUERY_MESSAGE.md` and the winning
copy does not: after the merge, the draft is in the surviving directory with its
original content.
Cover both directions (source newer, destination newer).
