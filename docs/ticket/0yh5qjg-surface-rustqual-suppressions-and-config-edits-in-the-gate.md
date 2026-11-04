# Surface rustqual suppressions and config edits in the gate

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-10-06
- **Label**: domain=tooling
- **Label**: type=task

With no committed baseline, the gate can only be silenced in two ways: a
`qual:allow` marker in the code, or an edit to the rustqual config.
Make both visible wherever the gate reports.

This is a rustqual feature, not a `qual` one.
It goes into the vendored rustqual (T-1dhk659) once its follow-up has folded
`qual` in, so it works in any workspace that compares against a base
revision.
As with every rustqual change, nothing JP-specific: the config files watched
are the ones the run reads, not a hard-coded `.config/rustqual/`.

## Behaviour, when comparing against a base revision

- List `qual:allow`, `qual:api` and `qual:test_helper` markers added since the
  base, in their own section, with file, line and reason.
- List changed config files in their own section: every file the run reads its
  configuration and policy from.
- Fail on any added `qual:allow` that has no `reason:`.
  Check whether rustqual already rejects a reasonless marker before adding a
  second check.
- Neither section fails the gate otherwise.
  They exist so a reviewer sees them, and so CI and `cargo_check` show them to
  the author.
- Configurable, as every rustqual check is.

## Assistant instructions

Update the `dev` and `rfd-implementor` personas, and the `coding` skill if it
covers verification:

- Fix the finding.
  Don't add a suppression, change a threshold, or edit `.config/rustqual/`
  unless the user asked for it in this conversation.
- If a finding looks wrong, say so and stop; don't route around it.
