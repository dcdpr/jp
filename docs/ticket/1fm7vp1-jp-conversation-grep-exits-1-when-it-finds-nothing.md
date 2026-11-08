# jp conversation grep exits 1 when it finds nothing

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-08
- **Label**: client=cli
- **Label**: domain=conversation
- **Label**: package=jp_cli
- **Label**: type=task

In JP, exit status 1 means an error and nothing else.
`jp conversation grep` breaks that rule twice:

- Finding nothing returns `Error::from(1).expected()`
  (`crates/jp_cli/src/cmd/conversation/grep.rs:420`, `:511`), so a script can't
  tell "no matches" from a failure.
- `pattern matching failed` exits 2 (`grep.rs:414`).
  That is the code clap, and plugin dispatch after it
  (`crates/jp_cli/src/cmd/plugin/dispatch.rs:3038-3041`), use for usage errors,
  but a pattern failing part-way through is a runtime error.

Proposed: "no matches" exits 3, the code the draft RFD *jp decide* (currently
D74) gives a below-threshold answer, so both commands report a negative result
the same way.
The pattern failure exits 1.

Scripts that branch on grep's exit 1 break, so the change goes in the changelog.

- [ ] "No matches" exits 3, still marked expected.
- [ ] A failed pattern exits 1.
- [ ] The docs that name the old codes: `render_empty`'s ("The status is always
      1") and `RunOutcome::AsExpected`'s (`crates/jp_cli/src/lib.rs:330-333`).
