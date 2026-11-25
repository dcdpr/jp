# --no-persist still writes during the startup scan

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-20
- **Label**: domain=storage
- **Label**: package=jp_cli
- **Label**: package=jp_storage
- **Label**: type=bug

Under `--no-persist`, the scan for conversations that do not load goes through
`DryRunRecovery`, which delegates `scan` to `FsStorageBackend`.
That runs `Storage::validate_conversations`, which writes to disk:

- `cleanup_import_staging` removes leftover `.import-*` staging directories;
- `cleanup_orphaned_tmp_files` removes orphaned `.tmp` files from conversation
  directories;
- both take conversation locks, creating and removing lock files;
- the scan cache is saved to user-local storage.

`jp conversation rm` under `--no-persist` then prints "no files on disk were
modified or removed", which is not strictly true.

The fix is a read-only scan in `jp_storage`: validation that reads and reports,
with the cleanup and the cache write as a separate step the persisting backend
runs.
`DryRunRecovery::scan` calls the read-only one.

- [ ] `Storage` can validate without removing files, taking locks, or saving the
      scan cache
- [ ] `DryRunRecovery::scan` uses it, with a test that a `--no-persist` run
      leaves a `.tmp` file, an import staging directory, and a missing scan
      cache as they were
- [ ] The persisting scan keeps its cleanup
