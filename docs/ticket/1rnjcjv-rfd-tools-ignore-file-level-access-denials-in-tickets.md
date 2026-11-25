# RFD tools ignore file-level access denials in tickets

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-25
- **Label**: domain=tooling
- **Label**: relevant-rfd=116
- **Label**: type=bug

The `rfd_*` tools check the call's access policy only per directory
(`Place::require` and `require_tickets` in
`crates/plugins/command/rfd/src/tool.rs`), then read every ticket, and `promote`
and `renumber` rewrite them too.
A policy that grants `docs/ticket` but denies one file under it is not honoured:
the denied file is still read, its path can appear in a warning, and a promotion
or renumber can rewrite it.

This predates PR #1392, which added a third reader of every ticket (the draft
slot scan in `cmd::draft_mentions`), and was raised in its review.

- [ ] Check each ticket file against the policy before reading or rewriting it,
      in `draft`, `promote`, `renumber`, and `rename`.
- [ ] Decide what a denied file means for the draft slot scan: skipping it can
      let a slot it cites be handed out, so refusing the call may be the safer
      answer.
- [ ] Add a tool test with a policy that denies one ticket file.
