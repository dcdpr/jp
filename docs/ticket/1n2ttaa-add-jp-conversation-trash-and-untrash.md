# Add jp conversation trash and untrash

- **Status**: Todo
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

- [ ] `trash` works on valid and invalid conversations, in either storage root
- [ ] `untrash` restores to the live partition and refuses to overwrite a live
      directory with the same id
- [ ] The end-of-command load-failure report names `jp c trash` as one way to
      clear an entry

## Comments

-----

- **From**: jp
- **Date**: 2026-11-18T19:53:26Z

`jp_storage::trash` (`trash_conversation`, `TRASH_DIR`, the `TRASHED.md` writer,
and their tests) was deleted with T-1n2tta8, because nothing called it any more.
Restore it from git history when `jp c trash` lands; the `TRASHED.md` wording
("failed workspace sanitization") should change for a user-initiated trash.
