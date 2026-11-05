# Changelog Fragments and Validation

- **Status**: In Progress
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-11-03
- **Label**: tracking-rfd=121
- **Label**: type=tracking

Tracking ticket for [RFD 121].

## Implementation plan

- [x] **Add fragment format and CI validation**: Create
      `crates/internal/changelog` with the fragment parser, upgrade-section
      handling, validation (released version directories included), and the
      `changelog` binary.
      Write `changelog.d/README.md`, add `just changelog-ci` to the `rust.yml`
      task matrix, and add the gate workflow with its presence and
      breaking-change rules.
      Mergeable on its own, so contributors can write fragments by hand from the
      day it lands.
- [x] **Add highlight note format**: Extend the parser and validation to cover
      `highlights/` notes.
      Depends on Phase 1.
- [x] **Build the `jp release` plugin**: Create `crates/plugins/command/release`
      with `init`, `check`, and `add`, and admit it in `.jp/config.toml`.
      `init` writes Phase 1's README into other workspaces.
      Serve `add` as the `changelog_add` tool with `tools init`, and enable it
      in the `dev` and `rfd-implementor` personas.
      Depends on Phase 1.
- [ ] **Auto-apply the no-changelog label (optional)**: Add the gate's
      best-effort step that sets `no-changelog` when a pull request opens, using
      a documented path classification (shipping, non-shipping, ambiguous,
      unlisted) that a human can override by toggling the label.
      Optional polish, since the gate works with manually applied labels.

[RFD 121]: ../rfd/121-changelog-fragments-and-validation.md
