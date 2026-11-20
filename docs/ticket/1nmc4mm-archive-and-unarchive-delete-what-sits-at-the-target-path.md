# Archive and unarchive delete what sits at the target path

- **Status**: Done
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

- [x] Archive and unarchive refuse when the target exists, naming both paths,
      instead of deleting it
- [x] `unarchive` holds the conversation lock across the move
- [x] Tests for both collisions and for a locked conversation

## Comments

-----

- **From**: jp
- **Date**: 2026-11-20T15:50:49Z

Fixed on `trash-rework2`.

- `archive_conversation` and `unarchive_conversation` share one move.
  It refuses with `Error::ConversationExists { id, from, existing }` when the
  target partition already holds a directory for the conversation under any
  title, not only the same name, since a second directory for one ID is the
  conflicting-copies case (T-1p3pk56).
- Every root is checked before anything moves, so a projected conversation is
  never left archived in one root and live in the other.
- The source and the collision check match the ID exactly (`<id>` or
  `<id>-<title>`).
  Both functions used a bare prefix match before, so `176362575260` counted as
  conversation `17636257526`.
- `Workspace::unarchive_conversation` holds the conversation lock across the
  move and the metadata write, and returns `LockFailed` when another process
  holds it.
  Both `jp c unarchive` and `jp c use archived` go through it.

Not changed: `load_archived_conversation_metadata` still uses the bare prefix
match when reading an archived conversation's metadata.
It only reads, so it is listed here rather than fixed in passing.
