# Distributing the jp Binary

- **Status**: In Progress
- **Authors**: jp
- **Date**: 2026-11-08
- **Label**: tracking-rfd=123
- **Label**: type=tracking

Tracking ticket for [RFD 123].

## Implementation plan

- [x] **Write the build script**: Turns the build spike into
      `.config/release/build.sh`: the five targets, the release toolchain from
      `[workspace.metadata.release]`, the `release` and `quick` profiles, the
      archives, `checksums.txt`, and `manifest.json`.
      Sets JP's `build` option and release toolchain, and makes `jp --version`
      read `SOURCE_DATE_EPOCH`.
      Depends on RFD 122's building phase.
- [x] **Add install scripts**: Adds `install.sh` and `install.ps1` to
      `.config/release/`, with the `{download}` placeholder a publisher fills in
      and `JP_DOWNLOAD` to try them, and replaces the "no binaries" warning in
      `docs/installation.md` with them, manual download and checksum
      verification, the macOS quarantine note, and building from source.
      Depends on Phase 1.
- [ ] **Add `jp release github`**: Creates the plugin with `publish`, its
      `assets` setting, and `sync`, adds GitHub's Releases API to `jp_github`,
      sets JP's `assets` to its install scripts, and runs `sync` in JP's CI on
      pushes to `main` that change a version's directory.
      Depends on Phases 1 and 2.
- [ ] **Release JP's history**: Adds `just backfill` with `init`, `next`,
      `proceed`, `revert`, and `publish`, and runs the trial; its `publish` is
      JP's first release with binaries.
      Depends on Phase 3 and RFD 122's phases for cutting the past, building,
      and links and drafts.

Before the history's `publish`, the `documentation` environment is restricted to
deployments from `main`; after it, [T-1f2jmb7]'s rulesets apply.

[RFD 123]: ../rfd/123-distributing-the-jp-binary.md
[T-1f2jmb7]: 1f2jmb7-protect-release-branches-and-release-tags-on-github.md
