# A reused draft slot inherits stale citations

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: domain=tooling
- **Label**: relevant-rfd=116
- **Label**: type=bug

Promoting a draft rewrites references to a *different* document when the draft's
slot previously belonged to another draft that is still cited in the repository.

Observed when D38 was promoted to RFD 140 (the review frontend): T-0571g2h cited
`[RFD D38]` for "root-qualified extends paths".
That earlier draft was created as D38 in `809bba6af` and renumbered to D61 in
`c078a9b8d`, leaving the citation in T-0571g2h pointing at `D38`.
`jp rfd draft` later allocated the free D38 slot to the review frontend, and
promoting it rewrote both citations in T-0571g2h to `[RFD 140]`, silently
pointing them at the wrong RFD.
(T-0571g2h has been fixed manually.)

Cause:

- `next_draft_slot` (`crates/plugins/command/rfd/src/id.rs`) treats a slot as
  free whenever no file in `drafts/` uses it, even if other documents still cite
  that draft ID.

- `draft_rules` (`crates/plugins/command/rfd/src/cmd/promote.rs`) rewrites every
  `Dnn` citation and token across all RFDs, drafts, and tickets without
  verifying which draft the citation referred to.

- [ ] Skip any draft slot in `jp rfd draft` that is still cited by an RFD,
      draft, or ticket.

- [ ] Report dangling draft citations (draft IDs with no matching file in
      `drafts/`) so they can be fixed before a slot is reused.
