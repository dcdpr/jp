# A managed conversation edit deletes the unsent query draft

- **Status**: Done
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-27
- **Label**: client=cli
- **Label**: domain=storage
- **Label**: package=jp_storage
- **Label**: type=bug

`jp conversation edit --events` (or `--metadata`, `--base-config`) on a
projected conversation silently deletes the conversation's `QUERY_MESSAGE.md`,
and reports success.

## Why

After a managed edit, `edit.rs` calls `FsStorageBackend::sync_projection` for
every edited conversation.
`Storage::sync_projection` (`crates/jp_storage/src/lib.rs`) removes the
user-local conversation directory with `remove_conversation_dirs`, then copies
the workspace copy into its place with `copy_dir_all`.

The query draft is written only to the user-local copy: the plugin host's
`draft_path` refuses the workspace tree, because the workspace copy is
committed.
So the draft is never in the copy the sync restores, and replacing the directory
drops it.
Any other file kept only in user-local storage is lost the same way.

## Scope

Contained to the conversation being edited, but hidden: the text the user had
composed and not sent is gone with no diagnostic.
Local-only conversations are unaffected, because the sync returns early when
there is no workspace copy.

## Fix

Make the sync replace the managed files, not the directory: bring the user-local
directory to the workspace copy's name (a `--metadata` edit can change the
title), then copy `metadata.json`, `events.json`, and `base_config.json` across.
That is the shape `persist_conversation_to` already writes each root in.

## Verifying

A regression through `sync_projection`: a projected conversation with a
`QUERY_MESSAGE.md` in its user-local directory, a changed `events.json` in the
workspace copy, then a sync.
Assert the events were synchronized and the draft is still there with its
original content.
A second case with a title change asserts the draft follows the renamed
directory.

## Related

RFD D72 introduces `metadata.local.json` beside the draft, which the same sync
would delete; its Phase 1 depends on this fix.
