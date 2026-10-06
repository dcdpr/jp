# Surface rustqual suppressions and config edits in the gate

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-10-06
- **Label**: domain=tooling
- **Label**: type=task

With no committed baseline, the gate can only be silenced in two ways: a
`qual:allow` marker in the code, or an edit under `.config/rustqual/`.
Make both visible wherever the gate reports.
Builds on T-0yh5frr.

## Gate behaviour (`qual diff`)

- List `qual:allow`, `qual:api` and `qual:test_helper` markers added since the
  base, in their own section, with file, line and reason.
- List changed files under `.config/rustqual/` in their own section.
- Fail on any added `qual:allow` that has no `reason:`.
- Neither section fails the gate otherwise.
  They exist so a reviewer sees them, and so CI and `cargo_check` show them to
  the author.

## Assistant instructions

Update the `dev` and `rfd-implementor` personas, and the `coding` skill if it
covers verification:

- Fix the finding.
  Don't add a suppression, change a threshold, or edit `.config/rustqual/`
  unless the user asked for it in this conversation.
- If a finding looks wrong, say so and stop; don't route around it.
