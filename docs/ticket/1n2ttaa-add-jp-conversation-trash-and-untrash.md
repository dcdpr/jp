# Add jp conversation trash and untrash

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-18
- **Label**: client=cli
- **Label**: domain=conversation
- **Label**: package=jp_cli
- **Label**: package=jp_storage
- **Label**: type=feature
- **Parent**: T-1n2tnwv

`jp c trash <id>` moves a conversation to `.trash/`, where it stays until
untrashed.
`jp c untrash <id>` moves it back.
Nothing else (grep, ls, query) looks inside the trash; live and archived
conversations are expected to be valid.

These two commands, with `jp c repair`, are the only ones that accept a
conversation that failed to load.
Every other command resolves ids through the conversation index, which excludes
them.
Target resolution for these commands reads the directory listing directly.

- [x] `trash` works on valid, invalid, and unstored (draft-only) conversations,
      in either storage root
- [x] `trash` and `untrash` accept several ids in one call
- [x] `untrash` restores to the live partition and refuses to overwrite a live
      directory with the same id
- [x] The end-of-command load-failure report names `jp c trash` as one way to
      clear an entry

## Comments

-----

- **From**: jp
- **Date**: 2026-11-18T19:53:26Z

`jp_storage::trash` (`trash_conversation`, `TRASH_DIR`, the `TRASHED.md` writer,
and their tests) was deleted with T-1n2tta8, because nothing called it any more.
Restore it from git history when `jp c trash` lands; the `TRASHED.md` wording
("failed workspace sanitization") should change for a user-initiated trash.

-----

- **From**: jp
- **Date**: 2026-11-19T13:44:44Z

Done.

- **Storage:** `jp_storage::trash` was rewritten rather than restored.
  The old code renamed a collision to `{dirname}-1`, which parses as ID plus
  title, so untrash could not tell copies apart.
  The trash now holds at most one entry per conversation per root.
  `trash` refuses a conversation the trash already holds (`AlreadyTrashed`), and
  `untrash` refuses when a live directory exists (`Exists`).
  Both hold the conversation lock, check before moving anything, and move every
  root's copy.
  `TRASHED.md` reads "Moved here by `jp conversation trash` on `<date>`.
  Run `jp conversation untrash <id>` to move it back."
  It doesn't contain the auto-trash sentence, so the one-time restore never
  picks it up (tested).
- **CLI:** `jp c trash <id>...` / `jp c untrash <id>...`, with at least one ID
  required.
  Typed IDs bypass the index; keywords and `-` (stdin) still resolve.
  The picker is refused, and `help` prints keyword help.
  Each failure is reported on stderr and the remaining IDs still run, then the
  command fails with "Failed to move N of M conversations to the trash."
  There is no confirmation prompt, since untrash reverses it.
  The command refuses to run under `--no-persist`.
- **Report hint:** when the load-failure report lists failures, its last line is
  ``Run `jp conversation trash <ids>` to move them to the trash.`` Directories
  whose name isn't an ID are left out of the command.
- **Glossary:** added *Trashed Conversation* to W-00dxybc.

Unchanged: the JSON load-failure records carry no hint.

-----

- **From**: jp
- **Date**: 2026-11-19T13:59:01Z
- **Re**: #2

Revised after text review:

- **Failure summary:** a failing run now ends with each failed conversation and
  its reason: `Failed to move N of M conversations to the trash:`, a blank line,
  then one `<id>:  <reason>` line per failure.
  IDs are bold yellow only under the pretty text format.
- **`--no-persist`:** a dry run instead of an error.
  It prints the same per-conversation lines, then a blank line and "This command
  ran under `--no-persist`, no files on disk were modified."
  Backed by `Storage::plan_trash` / `plan_untrash`, which run the same checks as
  the real operations.
  They check the lock without taking it, so nothing is written, not even a lock
  file.

-----

- **From**: jp
- **Date**: 2026-11-19T17:04:38Z

Withdrawn.
Once JP no longer moves conversations on its own, a trash only postpones a
decision, and archive already covers "keep it, out of the way" for conversations
that load.
`jp conversation trash`/`untrash`, `jp_storage::trash`, the trash picker, the
load-failure trash hint, and the *Trashed Conversation* glossary entry are
removed.

The part worth keeping, reaching conversations that fail to load by ID, moves to
`jp c rm`: T-1nmc61k.
The one-time restore of auto-trashed conversations (T-1n2tta9) stays.
