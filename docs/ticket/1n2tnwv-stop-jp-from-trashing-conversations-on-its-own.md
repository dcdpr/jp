# Stop JP from trashing conversations on its own

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-18
- **Label**: client=cli
- **Label**: domain=conversation
- **Label**: domain=storage
- **Label**: type=tracking

On every run, `Workspace::sanitize()` moves each conversation directory that
fails structural validation into `conversations/.trash/` without asking.
At default verbosity it prints nothing, because the only report is a `warn!`
trace.
Users can accumulate hundreds of auto-trashed directories (often marked `missing
metadata.json`) without knowing why or having a built-in command to restore
them.

This parent ticket reverses the auto-trash decision of RFD 052:

- **JP never moves user data on its own.** Startup validation reads and reports.
  It does not repair.
- **Report loudly and carry on.** Unloadable conversations are reported on
  stderr after the command's output.
  The command runs normally on the valid conversations.
- **Fixing is a user action.** `jp conversation repair` and `jp conversation rm`
  act only when asked, and `rm` reaches conversations that fail to load.
  `jp doctor` lists problems and suggests fixes.
  There is no trash: a conversation you want out of the way is removed, or moved
  out of the conversations directory by hand.
- **Invalid conversations stop at the edges.** They are caught where the store
  is scanned, and never enter the conversation index or reach core code.
  Defensive handling must not spread through the codebase.
- **Undoing what JP already did is temporary.** Restoring auto-trashed
  conversations lives in one isolated module in `jp_cli`, built to be deleted
  once users no longer have auto-trashed data.

Causes identified or suspected for `missing metadata.json` directories:

- `--no-persist` queries write a query draft (`QUERY_MESSAGE.md`) into user
  storage; a failed run leaves the file behind in a directory without metadata.
- A fresh conversation composes its draft in an untitled directory before its
  first write, so a failure or interrupt before the first flush leaves a
  draft-only directory.
- Trashed directories that already carry a title in their directory name are not
  explained by either draft-only path and still need investigation.

Done when every child is done.
Whether to backfill an RFD superseding RFD 052's auto-trash design is decided
afterwards.
