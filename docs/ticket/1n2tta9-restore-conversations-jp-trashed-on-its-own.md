# Restore conversations JP trashed on its own

- **Status**: Done
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

- [x] Runs once per storage root, gated by a marker file
- [x] Skips conversations whose lock is held, and retries them on a later run
- [x] Never overwrites a live directory with the same id
- [x] Prints what it moved and what it left, once

## Comments

-----

- **From**: jp
- **Date**: 2026-11-19T12:15:04Z

Done, in `jp_cli::trash_restore`.
The module and its call in `run_inner` are all that needs deleting later.

- **What counts as auto-trashed:** an entry whose `TRASHED.md` contains "failed
  workspace sanitization".
  Anything else in `.trash/` is left alone and not reported.
- **Classification:** uses the new
  `jp_storage::validate::validate_conversation_dir`, the same checks startup
  validation runs on one directory.
- **Live conversations:** an entry is left when a live directory for its ID
  exists in *either* storage root, not just its own.
  Moving a trashed copy beside a live projected one could make loading pick the
  trashed copy.
- **Marker:** `.trash/.auto-trash-restored`, written only once nothing in that
  trash is waiting on a lock or a failed move.
  Left entries are reported once and not looked at again.
- **When it runs:** after the workspace opens and before startup validation,
  never under `--no-persist`.
  Its report prints just before the load-failure report, past `--quiet`, and as
  one typed record per trash under `--format=json`.
- `TRASHED.md` is deleted from every entry moved back.

-----

- **From**: jp
- **Date**: 2026-11-20T11:44:13Z

Simplified.
With the trash gone as a concept, the restore moves every entry of each root's
`.trash/` back into `conversations/` and prints nothing: what still fails to
load shows up in the load-failure report, and what is not a conversation in `jp
doctor`.
`TRASHED.md` is removed from each moved entry, and `.trash/` itself once it is
empty, so no done-marker is needed.

Two cases stay in `.trash/`, logged with `warn!` and retried on every run: an
entry whose conversation another process holds, and an entry whose conversation
already has a live directory in either root (moving it would give one
conversation two directories).
The `TRASHED.md` filter, the validation checks, the restore report and its JSON
records are gone.
`jp doctor` (T-1n2ttac) lists what is left in an old `.trash/`.

-----

- **From**: jp
- **Date**: 2026-11-20T14:51:33Z

Reworked on `trash-rework2`.
The restore now fixes each entry inside `.trash/` before moving it out, and
takes no conversation lock (nothing reads or writes `.trash/`):

- `TRASHED.md` is removed; an entry left empty is deleted rather than moved
  back.
- A draft-only entry gets the files of a conversation with no turns, written
  with `jp_storage::write_conversation_files` into exactly that directory (the
  normal save path would rename and delete other directories for the same ID).
  It is dated by its ID and uses the workspace config as its base config.
- An entry whose ID is already live or archived comes back under the next free
  tenth of a second, checked against every live, archived and trashed directory
  in both roots.
  This also covers the `<id>-1` names older versions gave a second entry for one
  ID.
- Anything that still cannot be moved (a non-ID name that is taken, an I/O
  failure) stays in the trash and is listed in an end-of-run report on every
  run, in text and as `trash_entry_not_restored` JSON records.

`jp_cli::draft_migration` is gone; the restore covers its case.
