# Let jp c rm remove conversations that fail to load

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: client=cli
- **Label**: domain=conversation
- **Label**: package=jp_cli
- **Label**: package=jp_storage
- **Label**: type=feature
- **Parent**: T-1n2tnwv

`jp c rm` resolves IDs through the conversation index, which leaves out
conversations that fail validation and unstored (draft-only) directories.
Today the only way to remove one is `rm -rf`, possibly in both the workspace and
the user-local `conversations/`.

`rm` accepts such a conversation by ID, under a strict rule for what it may
delete:

- Only directories named for the typed ID: `<id>` or `<id>-<title>`, an exact
  match on the ID part.

- Only entries directly inside a storage root's live `conversations/`.
  Never dot-prefixed entries (`.archive`, `.trash`, `.import-*`,
  `.scan_cache.json`), never `conversations/` itself.

- Never a directory whose name is not a conversation ID; `jp doctor` shows its
  path for the user to handle.

- [x] `rm <id>` removes invalid and unstored conversations, in every storage
      root, under the rule above

- [x] Keeps `rm`'s confirmation prompt; a conversation that doesn't load is
      described by ID, path, and why it fails

- [x] Holds the conversation lock, and refuses a conversation another process
      holds

- [x] A dry run under `--no-persist` that prints what would be removed

- [x] The load-failure report ends with the `jp c rm <ids>` command for the
      listed conversations, until T-1n2ttac points it at `jp doctor`

- [x] Tests that `.archive/`, `.trash/`, and non-ID directories are never
      touched
