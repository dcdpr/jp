# Restore conversations JP trashed on its own

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-18
- **Label**: client=cli
- **Label**: domain=storage
- **Label**: package=jp_cli
- **Label**: type=task
- **Parent**: T-1n2tnwv

Every `.trash/` entry that exists today was moved there by startup sanitization;
each has a `TRASHED.md` saying it "failed workspace sanitization".
Restore them once automatically so users do not have to recover them by hand.

This migration is temporary: keep it in one isolated module in `jp_cli` so it
can be deleted cleanly once users no longer have auto-trashed data.

| Contents                              | Action                                                                       |
| ------------------------------------- | ---------------------------------------------------------------------------- |
| Passes validation today               | Move back, delete `TRASHED.md`                                               |
| Only a query draft                    | Move back; treated as pending (T-1n2tta7)                                    |
| Has `events.json`, no `metadata.json` | Move back; reported as a load failure, fixable with `jp conversation repair` |
| Anything else                         | Leave in `.trash/`, list in the report                                       |

- [ ] Runs once per storage root, gated by a marker file
- [ ] Skips conversations whose lock is held, and retries them on a later run
- [ ] Never overwrites a live directory with the same id
- [ ] Prints what it moved and what it left, once
