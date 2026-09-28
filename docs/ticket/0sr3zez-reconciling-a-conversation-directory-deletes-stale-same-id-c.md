# Reconciling a conversation directory deletes stale same-id copies with their drafts

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-27
- **Label**: domain=storage
- **Label**: package=jp_storage
- **Label**: type=bug

`reconcile_conversation_dir` (`crates/jp_storage/src/lib.rs`) renames one
existing copy to the target name when the target is missing, then runs
`fs::remove_dir_all` on every other directory for the same id.
A query draft or other non-managed file in one of those other copies is lost
without a diagnostic.

It runs on every persist (`persist_conversation_to`) and on every
`sync_projection`, so it is the common path.
The loss only happens when a root already holds more than one directory for the
id, for example after an interrupted title rename or a merge.
In a consistent store the rename branch carries the draft along and nothing is
deleted.

## Fix

Before removing a stale copy, move its non-managed files into the target when
the target lacks them.
When both hold the same non-managed file, keep the target's and log a warning
naming the dropped path.

## Verifying

A root holding both `<id>` and `<id>-title`, where only `<id>` has
`QUERY_MESSAGE.md`: after a persist with title `title`, the draft is in
`<id>-title` and `<id>` is gone.
