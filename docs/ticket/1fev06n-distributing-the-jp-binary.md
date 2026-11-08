# Distributing the jp Binary

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-08
- **Label**: tracking-rfd=123
- **Label**: type=tracking

Tracking ticket for [RFD 123].

## Implementation plan

- [ ] **Publish release notes on GitHub**: Adds `release-notes.yml`, started by
      the push of a release tag and by pushes to `main` that change a version's
      directory.
      It makes and publishes a Release for each tag, writes each description
      from `main` with building from source as the way to install, and marks the
      highest version as the latest.
      Depends on RFD 122 Phases 1, 2, and 4.
- [ ] **Rename the package and build with stable**: Renames the `jp_cli` package
      to `jp`, keeping its library name, and updates the places that name the
      package, including a one-time `cargo uninstall jp_cli` in the justfile's
      install step.
      Adds the release toolchain and `just release-build`, run in CI on pushes
      to `release/*` for each target.
      Worth landing before the first cut.
- [ ] **Build and upload binaries with dist**: Moves RFD 122's `release.yml` to
      `release-train.yml`, adds `dist-workspace.toml` and the `dist` profile,
      generates `release.yml`, pins its actions, and sets the toolchain through
      `github-build-setup`.
      `release-notes.yml` then runs as dist's plan job, making each Release a
      draft for dist to publish, and as its post-announce job, and its
      descriptions install with the release's scripts.
      Depends on Phases 1 and 2.
- [ ] **Write the installation guide**: Replaces the "no binaries" warning in
      `docs/installation.md` with the install scripts, checksum and attestation
      verification, the macOS quarantine note, and installing from source.
      Depends on Phase 3.

[RFD 123]: ../rfd/123-distributing-the-jp-binary.md
