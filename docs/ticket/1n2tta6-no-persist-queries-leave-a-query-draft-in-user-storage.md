# --no-persist queries leave a query draft in user storage

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-18
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: type=bug
- **Parent**: T-1n2tnwv

`jp --no-persist query --new "..."` writes `QUERY_MESSAGE.md` into a fresh
user-local `<id>/` directory (`preserve_query_message_file`, gated only on user
storage being configured).
The lock backend is null under `--no-persist`.
If the turn fails, the draft is kept for recovery.
`remove_empty_conversation_dir` leaves the non-empty directory alone, and the
next run trashes it as `missing metadata.json`.

Reproduced by `.config/loop/run.sh`'s rate-limit probe, which leaves one
directory per failed probe.

- [x] Failing test: a `--no-persist` inline query whose turn fails leaves no
      conversation directory
- [x] `--no-persist` never writes the preserved draft
- [x] Decide what an editor-composed `--no-persist` query does with its draft
      file

## Comments

-----

- **From**: jp
- **Date**: 2026-11-18T19:15:45Z

Decided: an editor-composed `--no-persist` query keeps its draft file on a
failed run, like any other query.
Losing typed text is worse than a leftover directory, and T-1n2tta7 makes a
draft-only directory read as an unstored conversation, so JP neither reports nor
moves it.
