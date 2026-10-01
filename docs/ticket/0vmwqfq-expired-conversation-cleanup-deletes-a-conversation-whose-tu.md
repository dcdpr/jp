# Expired-conversation cleanup deletes a conversation whose turn is still running

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-10-01
- **Label**: domain=plugins
- **Label**: domain=storage
- **Label**: package=jp_storage
- **Label**: package=jp_workspace

`Workspace::remove_ephemeral_conversations`
(`crates/jp_workspace/src/lib.rs:498`) removes every conversation whose
`expires_at` has passed, except the ids it is told to skip.
Its only caller (`crates/jp_cli/src/lib.rs:794-795`) skips the conversations
some session has active, from `all_active_conversation_ids`.
Nothing checks whether the conversation is locked: `PersistBackend::remove` goes
to `Storage::remove_conversation` (`crates/jp_storage/src/lib.rs:420`), which
deletes the directories outright.

`jp query --tmp` is not exposed, because its conversation is activated in the
session before the turn runs.
A conversation created by a plugin's `query` with `new` and `expires_in`
(T-0vmm72z) is never session-active.
While its turn runs, any other `jp` process that reaches teardown deletes it
once it has expired.
With `expires_in: "0s"` that is immediately; with `5m`, any turn that outlives
five minutes.
The turn then writes into a removed directory, or recreates a partial one, and
the plugin's `read_events` fails.

## Fix

Skip conversations whose lock is held, the way `sanitize` already does through
`Storage::is_conversation_locked`
(`crates/jp_storage/src/backend/fs.rs:294-296`).
The check belongs below the CLI, so every caller of the cleanup gets it: in
`remove_ephemeral_conversations`, or in `load_expired_conversation_ids` itself.

## Test

Lock an expired conversation through the workspace, run
`remove_ephemeral_conversations` with an empty skip list, and assert the
conversation is still on disk.
The existing `test_remove_ephemeral_conversations`
(`crates/jp_workspace/src/lib_tests.rs:704`) has the fixture.
