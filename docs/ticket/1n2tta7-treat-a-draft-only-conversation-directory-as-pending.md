# Treat a draft-only conversation directory as pending

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-18
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: package=jp_storage
- **Label**: type=bug
- **Parent**: T-1n2tnwv

A conversation directory holding only a query draft (`QUERY_MESSAGE.md`) and no
managed files is a conversation that was never stored, not a corrupt one.
A fresh conversation composes its draft in such a directory, and a failure or
interrupt before the first write leaves it behind on purpose, as the recovery
copy.

Validation classifies it as invalid (`missing metadata.json`), so today it gets
trashed.
Once validation reports failures instead of trashing them (T-1n2tta8), a
draft-only directory would show up as an error after every failed first query.

- [x] Validation returns a distinct outcome for draft-only directories, kept out
      of the index and out of the failure report
- [x] Test: a fresh query failing before the first write leaves a directory the
      next run neither reports nor moves

## Comments

-----

- **From**: jp
- **Date**: 2026-11-18T19:28:44Z

Done.
A conversation directory holding none of the managed files (`metadata.json`,
`base_config.json`, `events.json`) is an *unstored* conversation:
`ValidationResult::unstored`, never trashed, never reported, and skipped by the
index scan.
The rule is "no managed files", not "only `QUERY_MESSAGE.md`", because
`jp_storage` owns the managed files and doesn't know the draft's name; an empty
directory is unstored too.

The startup-path test
(`test_unstored_conversation_is_left_in_place_and_not_indexed` in
`jp_workspace`) builds the directory an interrupted `jp q --new` leaves rather
than driving a real interrupt.

The `jp doctor` item moved to T-1n2ttac, which needs
`FsStorageBackend::validate_conversations` (added here).

-----

- **From**: jp
- **Date**: 2026-11-20T15:44:58Z

Superseded on `trash-rework2`: the "unstored" outcome described above is gone.

A draft-only or empty conversation directory is now an ordinary invalid
conversation (`missing metadata.json`): it is reported by the load-failure
report and removable with `jp conversation rm`.
`ValidationResult::unstored`, `UnstoredConversation` and `DirContents` no longer
exist.
The test named above was removed with `Workspace::sanitize`;
`a_directory_without_conversation_files_is_invalid` in `jp_storage`'s
`validate_tests.rs` covers the case now.

The problem this ticket solved is handled at its source instead: `jp query`
stores a new conversation before writing its draft beside it, so a failed or
interrupted first query no longer leaves a draft-only directory.
Such directories now come only from older versions (the one-time trash restore
turns trashed ones into conversations with no turns), a crash between creating
the directory and the first write, or a directory made by hand.
A conversation another process holds the lock on is still left out of the
report, which covers one being created at that moment.
