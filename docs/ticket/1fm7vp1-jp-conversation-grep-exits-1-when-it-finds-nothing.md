# jp conversation grep exits 1 when it finds nothing

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-08
- **Label**: client=cli
- **Label**: domain=conversation
- **Label**: package=jp_cli
- **Label**: type=task

In JP, exit status `1` is reserved for runtime errors.
`jp conversation grep` violates that convention in two places:

- Returning no matches yields `Error::from(1).expected()`
  (`crates/jp_cli/src/cmd/conversation/grep.rs:420`, `:511`), preventing scripts
  from distinguishing "no matches" from an execution error.
- `pattern matching failed` exits with status `2` (`grep.rs:414`), which `clap`
  and plugin dispatch (`crates/jp_cli/src/cmd/plugin/dispatch.rs:3038-3041`)
  reserve for CLI usage errors, even though a mid-search pattern failure is a
  runtime error.

Proposal: exit `3` when no matches are found (matching the below-threshold exit
status in RFD 135 so both commands report negative outcomes with `3`), and exit
`1` on pattern matching failures.

Because scripts that branch on `grep` exiting `1` will need updating, this
change requires a changelog entry.

- [ ] Return exit status `3` (still marked expected) when no matches are found.
- [ ] Return exit status `1` when pattern matching fails.
- [ ] Update the doc comments on `render_empty` ("The status is always 1") and
      `RunOutcome::AsExpected` (`crates/jp_cli/src/lib.rs:330-333`).
