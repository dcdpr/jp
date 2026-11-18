# Treat a draft-only conversation directory as pending

- **Status**: Todo
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

- [ ] Validation returns a distinct outcome for draft-only directories, kept out
      of the index and out of the failure report
- [ ] `jp doctor` lists stale draft-only directories as unsent drafts
- [ ] Test: a fresh query failing before the first write leaves a directory the
      next run neither reports nor moves
