# A conversation removed or archived during a lock wait is recreated by the waiting writer

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-31

## What happens

A command that has already cached a conversation's metadata and events, then
waits for the flock, resurrects that conversation if the lock holder removed or
archived it while it waited.

Reproduction (two terminals, same conversation):

1. `jp c compact ID` — resolves conversation config, which eager-loads metadata
   and events into `Workspace::state`, then blocks on the flock.
2. `jp c rm ID` — holds the flock across `remove_conversation_with_lock`
   (`jp_cli/src/cmd/conversation/rm.rs:102-109`), deletes the conversation from
   both storage roots, releases.
3. Terminal 1 acquires the flock.
   `Workspace::sync_conversation` re-reads both loads, both report
   `LoadErrorInner::is_missing()`, and the `is_missing` arms keep the cached
   copy.
   Compact proceeds and its `flush()` writes the whole conversation back.

The deleted conversation reappears on disk with its pre-wait content.
Nothing tells the user.

## Archive variant

Archiving produces a mixed state rather than a clean resurrection, because the
two loads disagree about the archive partition:

- `FsStorageBackend::load_conversation_metadata` falls back to the archive
  (`jp_storage/src/backend/fs.rs:240`), so metadata refreshes successfully and
  brings `archived_at` with it.
- `load_conversation_stream` has no such fallback
  (`jp_storage/src/backend/fs.rs:250-255`), so the cached stream is kept.

The waiting writer then persists into the *live* partition, producing a live
conversation carrying `archived_at`.

## Not a regression

This predates the lock-refresh work in \#1045.
Before that change, `maybe_init_conversation` returned early on an
already-populated cell, so the cached copy survived the deletion and the flush
recreated it — the same outcome, reached by omission instead of by an explicit
arm.

## Why it was not fixed alongside the refresh

Preserving cached data on a missing load is correct for exactly one case: a
conversation created in memory and never persisted (`--no-persist`, the
in-memory backend, `create_conversation` in tests).
Distinguishing that from "another process deleted it" needs provenance we do not
record:

- `state.presence` has an entry for both —
  `create_conversation_with_projection` inserts one.
- `state.conversations` membership likewise covers both.

So the fix needs new state tracking whether a cached value came from the loader
or from an in-process create, with a lifecycle spanning create → first
successful persist.
That touches the highest-risk path in `jp_workspace` and deserves its own
change.

## Sketch

Record ids seeded by `create_conversation_with_projection` in a
`never_persisted: HashSet<ConversationId>`, and clear an id on its first
successful `PersistBackend::write`.
`sync_conversation` then keeps the cached copy on a missing load only for ids in
that set, and returns the missing error otherwise.
Note `sync_conversation` takes `&self`, so the set needs interior mutability or
the persist path needs to own it.

Regression tests: a removal and an archive completed by a second workspace
between the cached read and `lock_conversation`, asserting the acquisition fails
and the conversation stays gone.
