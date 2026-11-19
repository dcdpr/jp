# Archive and unarchive delete what sits at the target path

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: client=cli
- **Label**: domain=storage
- **Label**: package=jp_storage
- **Label**: type=bug

`Storage::archive_conversation` and `Storage::unarchive_conversation`
(`crates/jp_storage/src/lib.rs`) run `fs::remove_dir_all` on the target
directory when it already exists, then rename into it.
A stale or duplicate copy at the target, in `.archive/` or among the live
conversations, is deleted without a diagnostic.
That is the kind of silent data removal T-1n2tnwv set out to end.

Separately, `jp c unarchive` moves the conversation without taking its lock, so
it can race another process writing to it; `archive` takes the lock.

- [ ] Archive and unarchive refuse when the target exists, naming both paths,
      instead of deleting it
- [ ] `unarchive` holds the conversation lock across the move
- [ ] Tests for both collisions and for a locked conversation
