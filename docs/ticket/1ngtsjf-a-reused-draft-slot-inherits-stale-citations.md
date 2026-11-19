# A reused draft slot inherits stale citations

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: domain=tooling
- **Label**: relevant-rfd=116
- **Label**: type=bug

Promoting a draft rewrites references to a *different* document when the draft's
slot once belonged to another draft that is still cited somewhere.

Seen when D38 was promoted to RFD 140 (the review frontend): T-0571g2h cited
`[RFD D38]` for "root-qualified extends paths".
That draft was created as D38 in `809bba6af` and renumbered to D61 in
`c078a9b8d`, which left the ticket's citation dangling.
`jp rfd draft` later gave the empty D38 slot to the review frontend, and
promoting it turned the ticket's two citations into `[RFD 140]`, pointing them
at the wrong RFD with no warning.
(The ticket is fixed by hand.)

Cause:

- `next_draft_slot` (`crates/plugins/command/rfd/src/id.rs`) counts a slot as
  free when no file in `drafts/` takes it, even if documents still cite it.

- `draft_rules` (`crates/plugins/command/rfd/src/cmd/promote.rs`) rewrites the
  bare `Dnn` citation and token in every RFD, draft, and ticket, whether or not
  the reference means the file being promoted.

- [ ] `jp rfd draft` skips a slot that any RFD, draft, or ticket still cites.

- [ ] Report citations of a draft id that no file has, so they get fixed instead
      of adopted.
