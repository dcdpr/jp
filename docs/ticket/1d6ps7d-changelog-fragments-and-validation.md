# Changelog Fragments and Validation

- **Status**: Done
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
      with `init`, `check`, a command per type, and `highlight`, and admit it in
      `.jp/config.toml`.
      `init` writes Phase 1's README into other workspaces.
      Serve writing a fragment as the `changelog_add` tool, with `tools init`.
      Depends on Phase 1.

The repository settings the plan names are T-1fev06m, and applying the
`no-changelog` label automatically, the optional fourth phase, is T-1fets19.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-08T00:10:46Z

Phase 4, applying the `no-changelog` label automatically, moved to T-1fets19 so
RFD 121 could be marked Implemented without it; the RFD now mentions that ticket
instead of planning the phase.
The repository settings the plan names are T-1fev06m.
The plugin's item names the command per type and `highlight`, which replaced
`add` while RFD 122 was implemented.

[RFD 121]: ../rfd/121-changelog-fragments-and-validation.md
