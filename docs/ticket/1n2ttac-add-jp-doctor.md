# Add jp doctor

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-18
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: type=feature
- **Parent**: T-1n2tnwv

Check the `jp` installation, user-global and workspace configuration, and
conversation storage (load failures, unsent drafts, and `.trash/` contents),
reporting each detected problem with a suggested fix.
`jp doctor` is read-only: it suggests commands such as `jp c repair` and `jp c
trash` without modifying anything on disk.

- [ ] Lists stale unstored conversation directories (no managed files, usually
      a query draft) as unsent drafts, from
      `FsStorageBackend::validate_conversations`
- [ ] The load-failure report printed after every command
      (`jp_cli::load_failure::render`) points at `jp doctor`, replacing its
      "Run with `-v`" hint
