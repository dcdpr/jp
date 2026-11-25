# RFD tools ignore file-level access denials in tickets

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-25
- **Label**: domain=tooling
- **Label**: relevant-rfd=116
- **Label**: type=bug

The `rfd_*` tools check the call's access policy only at the directory level
(`Place::require` and `require_tickets` in
`crates/plugins/command/rfd/src/tool.rs`), then read every ticket file, and
`promote` and `renumber` rewrite them as well.
A policy that grants access to `docs/ticket` but denies a specific file under it
is not enforced: the denied file is still read, its path can appear in a
warning, and `promote` or `renumber` can rewrite it.

This predates PR #1392, which added a third scan of all tickets
(`cmd::draft_mentions`), and was raised during its review.

- [ ] Check each ticket file against the access policy before reading or
      rewriting it in `draft`, `promote`, `renumber`, and `rename`.
- [ ] Decide how the draft slot scan should handle a denied ticket file:
      skipping it could allocate a slot that the denied file cites, so refusing
      the tool call may be safer.
- [ ] Add a tool test with a policy that denies a single ticket file.
