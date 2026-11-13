# Distributing the jp Binary

- **Status**: In Progress
- **Authors**: jp
- **Date**: 2026-11-08
- **Label**: tracking-rfd=123
- **Label**: type=tracking

Tracking ticket for [RFD 123].

## Implementation plan

- [x] **Write the build script**: Add `.config/release/build.sh` supporting all
      five targets, `[workspace.metadata.release].toolchain`, the `release` and
      `quick` profiles, archive packaging, `checksums.txt`, and `manifest.json`.
      Configure JP's `build` option and release toolchain, and update `jp
      --version` to read `SOURCE_DATE_EPOCH`.
      Depends on RFD 122's building phase.
- [x] **Add install scripts**: Add `install.sh` and `install.ps1` to
      `.config/release/` with the `{download}` placeholder and `JP_DOWNLOAD`
      override, and update `docs/installation.md` with install script
      instructions, manual download and checksum verification, the macOS
      quarantine note, and building from source.
      Depends on Phase 1.
- [x] **Add `jp release github`**: Create
      `crates/plugins/command/release-github` with `publish` (supporting
      `assets`, `upgrade_guide`, and `footer`) and `sync`, add GitHub's Releases
      API to `jp_github`, configure `release-github` in `.jp/config.toml`, and
      run `sync` in JP's CI on pushes to `main` that modify `changelog.d/*/**`.
      Depends on Phases 1 and 2.
- [ ] **Release JP's history**: Add `just backfill` with `init`, `next`,
      `proceed`, `revert`, `publish`, and `draft`, and execute the backfill
      trial to publish JP's historical releases.
      Depends on Phase 3 and RFD 122 Phases 5 to 7.

Before running `just backfill publish`, restrict the `documentation` environment
to deployments from `main`; afterwards, apply the rulesets in [T-1f2jmb7].

[RFD 123]: ../rfd/123-distributing-the-jp-binary.md
[T-1f2jmb7]: 1f2jmb7-protect-release-branches-and-release-tags-on-github.md
