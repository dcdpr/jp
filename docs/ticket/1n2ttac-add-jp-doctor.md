# Add jp doctor

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-18
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: type=feature
- **Parent**: T-1n2tnwv

Check the `jp` installation, user-global and workspace configuration, and
conversation storage (load failures, unsent drafts, and entries the one-time
restore left in an old `.trash/`), reporting each detected problem with a
suggested fix.
`jp doctor` is read-only: it suggests commands such as `jp c repair` and `jp c
rm` without modifying anything on disk.

- [ ] Lists conversations with no turns and a query draft as unsent drafts, as
      a notice: what a run interrupted before its turn leaves, and what the
      one-time trash restore made of the draft-only directories older versions
      trashed
- [ ] Suggests removing all listed unsent drafts with one command,
      `jp conversation rm <id> <id> ...`, and shows each draft's first line so
      the user sees what they would lose
- [ ] Validates the archive partition (`conversations/.archive/`), which startup
      validation never reads, and reports archived conversations that fail to
      load
- [ ] Lists directories under `conversations/` whose name is not a conversation
      ID (`ValidationResult::unrecognized`), which the load-failure report
      leaves out, suggesting `rm -r <path>` for each
- [ ] The load-failure report printed after every command
      (`jp_cli::load_failure::render_text`) lists at most 5 failures again, and
      above that ends with "Run `jp doctor` to list them"
