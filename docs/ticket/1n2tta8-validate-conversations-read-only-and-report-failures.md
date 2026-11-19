# Validate conversations read-only and report failures

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-18
- **Label**: client=cli
- **Label**: domain=storage
- **Label**: package=jp_cli
- **Label**: package=jp_storage
- **Label**: package=jp_workspace
- **Label**: type=enhancement
- **Parent**: T-1n2tnwv

Startup validation moves invalid conversations to `.trash/`.
It should leave the filesystem untouched and report failures instead.

- [x] `FsStorageBackend::sanitize` no longer calls `trash_invalid_conversation`;
      the report lists invalid conversations instead of trashed ones
- [x] Invalid conversations never enter the conversation index or reach core
      code
- [x] Expiry cleanup never removes an invalid conversation
- [x] After the command's output, on stderr: up to 3 failures each with id,
      path, and error; more than 3 summarized as "N conversations failed to
      load"
- [x] Works the same under `--no-persist` and for read-only (`Inspect`) loads
- [x] Glossary entry updated (W-00dxybd says sanitization moves broken
      conversations to the trash)

## Comments

-----

- **From**: jp
- **Date**: 2026-11-18T19:53:26Z

Done.

- **Edge:** the storage index (`Storage::load_conversation_index`) runs the same
  structural check validation does, through the scan cache, so a broken
  conversation is left out for every reader, including the macOS app and others
  that never sanitize.
  In the CLI, startup validation has just warmed the cache, so the index costs a
  file stat per directory.
- **Expiry:** `load_expired_conversation_ids` considers only conversations that
  pass validation.
  Before, an expired conversation missing its `events.json` was deleted.
- **Report:** `jp_cli::load_failure::render`, printed through
  `Printer::error_println` after the command's output on every exit from the
  run.
  It goes to stderr past `--quiet`, and as a `{"message": ...}` record under
  `--format=json`.
  Above 3 failures it says "Run with `-v` to list them"; `-v` shows one warning
  per failure.
  Pointing at `jp doctor` moved to T-1n2ttac, since the command doesn't exist
  yet.
- **Trash code removed:** `trash_conversation` had no callers left, so
  `jp_storage::trash` is deleted.
  T-1n2ttaa can restore it from this commit's parent.

Not covered here: `reconcile_conversation_dir` deletes every other directory for
a conversation's ID when it persists, including a broken copy JP now reports.
That is T-0sr3zez.

A known gap: a projected conversation whose workspace copy is broken but whose
user-local copy is valid stays indexed.
If the broken copy has the newer mtime, loading picks it and fails.
The report names the broken path.

-----

- **From**: jp
- **Date**: 2026-11-19T11:50:51Z
- **Re**: #1

Revised after review of the terminal text:

- **Text:** the path is dropped.
  The count and each conversation ID are bold yellow.
  A broken file now reads `metadata.json: invalid JSON at line 1, column 2`.
- **JSON:** each failure is one typed record on stderr, not a `{"message": ...}`
  wrapper.
  Its fields are `level`, `code` (`conversation_load_failed`), `message`,
  `context.conversation` (`id`, `dirname`, `path`), and `causes`, the error
  chain one step per entry.
  A cause's `code` is one of `invalid_dirname`, `file_missing`,
  `file_unreadable`, `io_error`, or `json_invalid`; `json_invalid` carries
  `line` and `column`.
- **Typed errors:** `jp_storage::validate::ValidationError` has three variants:
  `InvalidDirname`, `MissingFile { file }`, and `UnreadableFile { file, source
  }`, with `ManagedFile` and `FileError` both typed.
  Neither `jp c repair` nor the report has to parse error strings.
- **`--quiet`:** its help states that load failures are still reported.
