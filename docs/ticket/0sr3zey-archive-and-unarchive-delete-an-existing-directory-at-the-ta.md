# Archive and unarchive delete an existing directory at the target name

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-27
- **Label**: domain=storage
- **Label**: package=jp_storage
- **Label**: type=bug

`archive_conversation` and `unarchive_conversation`
(`crates/jp_storage/src/lib.rs`) run `fs::remove_dir_all` on the target when a
directory of the same name is already there, then rename the source over it.
Any file in the replaced directory that the source lacks is lost without a
diagnostic.

## When it happens

- Archive: `.archive/<dirname>` already exists, for example after an earlier
  unarchive failed in one root but not the other, or when a teammate committed
  an archived copy.
- Unarchive: a live `<dirname>` already exists alongside the archived copy.
  One plausible route is a query draft written for the conversation while it was
  archived, if `draft_path(create = true)` derives a live path for it.
  I haven't confirmed that route; confirm it before writing the test.

## Fix

Treat a collision as a merge rather than a replacement: move the source's
managed files over the target's (the same rule `sync_projection` follows with
`MANAGED_FILES`), and keep target-only files.
If the collision is judged impossible in a consistent store, return an error
instead of deleting, so the state gets reported and not silently resolved.

## Verifying

For each direction, a pre-existing target directory holding a `QUERY_MESSAGE.md`
the source lacks: after the operation, the draft is still there.
