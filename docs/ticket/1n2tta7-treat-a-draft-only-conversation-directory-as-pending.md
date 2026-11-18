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
