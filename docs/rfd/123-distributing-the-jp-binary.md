# RFD 123: Distributing the jp Binary

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-14
- **Requires**: [RFD 122]
- **Summary**: Each release tag gets a GitHub Release with its notes, and the
  prebuilt, attested `jp` binaries and install scripts dist builds for it.

## Summary

Every release tag gets a GitHub Release, which JP's own workflow creates and
describes with the release notes rendered from `main`, as the documentation site
renders them.
[dist] (the `cargo-dist` crate) builds prebuilt `jp` binaries for Linux, macOS,
and Windows, uploads them to that Release alongside SHA-256 checksums, GitHub
artifact attestations, and install scripts, and publishes it.
Release notes come first: releases get a GitHub Release with their notes before
`dist` is set up, and binaries once it is.
This RFD covers only the `jp` binary; command plugins, the macOS app, the web
client, and macOS notarization are out of scope.

## Motivation

Installing JP with `cargo install --git` requires a Rust toolchain and several
minutes of compilation.
[RFD 122] cuts and tags a release every two weeks, but a tag alone gives users
nothing to download.
The first official release needs prebuilt binaries.

[RFD 122]'s commands end at a pushed tag and talk to no forge, so everything
GitHub shows of a release happens here, in JP's CI: the Release that announces
it, and the binaries built for it.

Cross-platform binary releases are a solved problem, and JP should use an
existing tool rather than maintain a custom workflow.
`plugins.yml` shows the cost of a hand-rolled workflow: maintaining a runner
matrix, renaming binaries, computing checksums in shell scripts, and uploading
assets one by one.
Using a dedicated release tool also provides standard checksum files, build
provenance attestations, and platform-detecting install scripts.

This RFD scopes distribution to the `jp` binary.
Command plugins will be distributed in a later RFD, and the macOS app and web
client remain unreleased prototypes.

## Design

### Release Assets

When a maintainer pushes a release tag with `jp release finalize` ([RFD 122]),
the workflow publishes a GitHub Release with the following assets:

| Asset                                 | Description                                                     |
| ------------------------------------- | --------------------------------------------------------------- |
| `jp-x86_64-unknown-linux-gnu.tar.xz`  | Target archive containing the `jp` binary, README, and license  |
| `jp-aarch64-unknown-linux-gnu.tar.xz` |                                                                 |
| `jp-aarch64-apple-darwin.tar.xz`      |                                                                 |
| `jp-x86_64-apple-darwin.tar.xz`       |                                                                 |
| `jp-x86_64-pc-windows-msvc.zip`       |                                                                 |
| `*.sha256`, `sha256.sum`              | Per-archive SHA-256 checksum files and a combined checksum file |
| `jp-installer.sh`, `jp-installer.ps1` | Install scripts for Unix shells and PowerShell                  |
| `dist-manifest.json`                  | Machine-readable manifest of the release assets                 |

Asset filenames omit the version number, so
`releases/latest/download/jp-installer.sh` always resolves to the newest
release.
Each archive also receives a GitHub artifact attestation: a Sigstore-signed
record of the workflow run and commit that built it, stored by GitHub rather
than uploaded as a release asset.

The GitHub Release is titled like the tag's subject, `Release 0.3.0`, and its
description contains the release notes rendered from the release's directory on
`main` (see [Release Notes](#release-notes)), followed by installation
instructions.

The target list matches the four targets built by `plugins.yml`, plus
`aarch64-unknown-linux-gnu` for ARM64 servers and Linux containers on Apple
Silicon.

### Installation

```sh
# Linux and macOS
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/dcdpr/jp/releases/latest/download/jp-installer.sh | sh
```

```powershell
# Windows
powershell -ExecutionPolicy Bypass -c "irm https://github.com/dcdpr/jp/releases/latest/download/jp-installer.ps1 | iex"
```

The install scripts detect the host platform, download the matching archive, and
install `jp` into Cargo's bin directory (`~/.cargo/bin`), matching `cargo
install`.
Users can also download an archive manually, verify its checksum with `sha256sum
-c sha256.sum`, and verify which workflow built it with `gh attestation verify`.
Building from source with `cargo install --git` remains supported.

macOS binaries are not notarized.
Gatekeeper enforces notarization only on files carrying the
`com.apple.quarantine` extended attribute, which web browsers set on downloads
but `curl` and the install script do not.
Only binaries downloaded through a browser are blocked, and running `xattr -d
com.apple.quarantine jp` allows them to run.
The installation guide documents this step, and a later RFD will add Developer
ID signing and notarization.

### Packaging with `dist`

[dist] (the `cargo-dist` crate) builds a Cargo workspace's binaries on GitHub
Actions runners and publishes them to a GitHub Release.
JP configures it in `dist-workspace.toml`:

```toml
[workspace]
members = ["cargo:."]

[dist]
cargo-dist-version = "0.33.0"
ci = ["github"]
packages = ["jp"]
precise-builds = true
targets = [
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-pc-windows-msvc",
]
installers = ["shell", "powershell"]
github-attestations = true
source-tarball = false
pr-run-mode = "skip"
create-release = false
plan-jobs = ["./release-notes"]
post-announce-jobs = ["./release-notes"]
github-build-setup = "build-setup.yml" # steps that select the release toolchain

[dist.github-custom-runners]
aarch64-unknown-linux-gnu = "ubuntu-22.04-arm"

[dist.github-action-commits]
# Every action pinned to a commit, as JP's other workflows are.
```

- `packages = ["jp"]` restricts builds to the `jp` package, ignoring all other
  workspace crates (including plugins) regardless of their `publish` setting.
- `precise-builds = true` builds only the `jp` package (`cargo build -p jp`),
  matching `cargo install`, instead of `dist`'s default of building the entire
  workspace with unified Cargo features.
- `source-tarball = false` omits `dist`'s `source.tar.gz` asset; GitHub
  automatically attaches source archives to every release, keeping the release
  assets limited to the binaries, installers, and checksums.
- `pr-run-mode = "skip"` disables `dist` on pull requests; the [release-branch
  check](#release-branch-build-check) validates release builds instead.
- `github-action-commits` pins every GitHub Action in `dist`'s generated
  workflow to a commit SHA.
- `create-release = false` leaves the GitHub Release to JP: `dist` uploads to
  the draft that `release-notes.yml` creates and publishes it once every upload
  is done (see [Release Notes](#release-notes)).
- `plan-jobs` and `post-announce-jobs` run `release-notes.yml` at the start of
  `dist`'s run, so the draft exists before anything is uploaded, and again after
  `dist` publishes the Release.

`dist` owns the workflow it generates at `.github/workflows/release.yml` and
has no setting for another name, so [RFD 122]'s workflow of that name (the
release reminder and the site build on a tag) moves to `release-train.yml`.
`dist generate` writes the file from `dist-workspace.toml`, and `dist plan`
fails if the workflow is out of sync with the configuration, so
`dist-workspace.toml` is the source of truth and `release.yml` is never edited
manually.
The workflow runs when a version tag is pushed: `dist` builds the packages whose
version matches the tag (`jp`, updated on the release branch by [RFD 122]'s
`bump` command), compiles each target on a native runner, generates
attestations, uploads the archives to the tag's draft Release, and publishes it.

Because a maintainer pushes the tag directly rather than from a workflow, the
release workflow starts automatically without manual approval (see [RFD 122]'s
JP's CI section).

### Renaming the `jp_cli` Package to `jp`

`dist` names archives and install scripts after the Cargo package.
Because the `jp` binary is built by the `jp_cli` package, `dist` would publish
`jp_cli-installer.sh`.
To avoid baking `jp_cli` into public install commands and scripts, the Cargo
package is renamed to `jp` before the first release, while `[lib] name =
"jp_cli"` preserves the library crate name.
Only references to the Cargo package name need updating:

- `crates/jp_cli/Cargo.toml` and its entry in `Cargo.lock`.
- `--package jp_cli` in the justfile's recipes that build or run `jp`, such as
  `run`, `serve-web`, and `release`.
- The `debug_jp` tools in `.config/jp/tools`, which build with `-p jp_cli`.
- The `cargo install` command in `README.md` and `docs/installation.md`.
- The example in the `cargo_check` tool's description.
- [RFD 122]'s `bump` command, which becomes `--package jp`.

References to the library crate or directory are unchanged: the `crates/jp_cli/`
directory, `jp_cli::` Rust paths, tracing targets and `RUST_LOG` filters, insta
snapshot names, and file paths in personas and tool documentation.
The `jp` binary and its `--help` and `--version` output are also unchanged
because clap's command name is set to `jp` explicitly rather than derived from
the package name.
The ticket label `package=jp_cli` keeps its value as well; it identifies code
under `crates/jp_cli` and is not checked against Cargo metadata.

Cargo records which package owns each installed binary in `~/.cargo/bin/jp`.
On machines where `jp` was installed from `jp_cli` with `cargo install`
(including contributor machines via `just install`), installing the `jp` package
fails until `jp_cli` is uninstalled.
The justfile's `install` recipe runs `cargo uninstall jp_cli` whenever `jp_cli`
is still listed as installed, and the installation guide instructs users who
built from source to do the same.

### Release Toolchain

`rust-toolchain.toml` pins a nightly compiler for development while specifying
that release binaries are built with stable Rust.
The release workflow sets `RUSTUP_TOOLCHAIN` to a stable version via `dist`'s
`github-build-setup` steps; that version is defined in one place in the justfile
alongside the other pinned tool versions.
Because regular CI builds use nightly, stable-compiler breakages would otherwise
go unnoticed until a release is tagged; the release-branch build check catches
them when the release branch is cut.

### Release-Branch Build Check

`just release-build` runs `dist build` for the host target with the release
toolchain, exercising the same tool, configuration, and compiler as a tagged
release build on a single machine.
JP's CI runs `just release-build` on each target's runner on every push to a
`release/*` branch, and contributors can run it locally without GitHub.
If a release branch fails to compile or package for any platform, CI fails
before `jp release finalize` ([RFD 122]) tags the release.

### Release Notes

JP's own workflow, `release-notes.yml`, owns each GitHub Release; `dist` only
uploads to it.
It is a reusable workflow, and each run brings the GitHub Releases in line with
`main`:

- Every release tag has a Release, titled `Release 0.3.0`.
  A Release the workflow makes starts as a draft, which dist publishes once its
  uploads are done; before dist is set up, the workflow publishes it at once,
  with no binaries.
- Each Release's description is the release's notes, rendered from
  `changelog.d/<version>/` on `main` with its breaking entries linking to the
  upgrade guide on JP's site, followed by how to install that release: building
  it from source with its tag until binaries ship, and its own install scripts
  once they do.
- The release with the highest version is GitHub's latest.

The workflow runs as `dist`'s plan job, so the draft exists before anything is
uploaded; as its post-announce job, once `dist` has published the Release; and
on every push to `main` that modifies a version's directory, so edits to an
older release's notes on `main` propagate to its GitHub Release.
Before `dist` is set up, pushing a release tag starts it directly.
Like the docs build ([RFD 122]'s On the site section), the workflow renders
notes with the `changelog` binary using the forge and headings from the settings
snapshot, compiling only that small crate rather than `jp`.

The description follows `main`, as the site does, while the tag keeps the notes
the release was tagged with ([RFD 122]'s Finalizing a release).
It links to the upgrade guide on the site rather than holding it, as the tag's
message does: GitHub gives a Release's description no heading anchors, so links
to its own sections would go nowhere ([GitHub discussion 48311]).

GitHub always has a `latest` release, which controls what
`releases/latest/download/` serves.
GitHub's API documentation states that publishing a release marks it as
`latest` unless the request says otherwise, whereas `gh` describes the default
as "automatic based on date and version".
Because a patch release on an older line (such as 0.3.1) can be published after
a newer release (0.4.0; see [RFD 122]'s Releases in parallel section), the API's
behavior would mark 0.3.1 as `latest` and cause the install command to install
0.3.1 instead of 0.4.0.
Marking the highest version as `latest` on every run is correct under either
behavior.

A release whose notes have not reached `main`, such as one finalized from a
branch ahead of it, gets a description saying they are on their way, until they
reach `main` and the workflow writes them.

GitHub's immutable releases lock a published Release's tag and assets, but not
its description.
This ordering works with them enabled: binaries are uploaded to the draft before
it is published, and descriptions are rewritten afterwards.

## Drawbacks

- **Generated workflow.** `release.yml` is generated by `dist` rather than
  written by hand, and its steps invoke `dist` directly instead of running
  `just` recipes.
  `just release-build` reproduces the build locally.
- **Additional tool to maintain.** `cargo-dist-version` pins `dist`, and
  upgrading it requires regenerating `release.yml` and reviewing the diff.
- **Experimental features.** `github-attestations` and `github-build-setup` are
  marked experimental in `dist`.
- **Attestation verification requires GitHub's CLI.** Checksums require only
  `sha256sum`, whereas verifying artifact attestations requires `gh`.
  Forge-independent signatures are deferred to future work.
- **Package rename migration.** Renaming `jp_cli` to `jp` updates several
  package references and requires everyone who installed `jp` via `cargo
  install` to uninstall `jp_cli` once.
- **Unsigned binaries.** Windows may warn when running an unsigned binary
  downloaded through a browser, and macOS blocks a browser-downloaded binary
  until its quarantine attribute is removed.
- **Two workflows meet at a draft.** dist uploads to the Release
  `release-notes.yml` makes at the start of its run; if that job fails, so do
  the uploads, until dist's workflow runs again.
- **Releases before dist stay without binaries.** With immutable releases on, a
  Release published before Phase 3 can't take assets later; installing one of
  those releases means building it from source.

## Alternatives

- **[upload-rust-binary-action] with [create-gh-release-action].** These
  well-maintained GitHub Actions build one binary per matrix job and upload it
  with checksums, leaving JP to write and maintain the surrounding workflow: the
  runner matrix, attestations, release notes, and install scripts.
  They also run only inside GitHub Actions.
  This is the preferred fallback if `dist`'s generated workflow becomes a
  burden.
- **[GoReleaser]'s Rust builder.** GoReleaser builds every target from a single
  machine and runs locally, cross-compiling with `cargo-zigbuild`.
  Cross-compilation is least reliable for macOS and Windows MSVC, where JP's
  dependencies use platform-specific APIs, and JP's CI already builds natively
  on all three platforms.
  It would also add a Go tool to a Rust release pipeline.
- **Extending `plugins.yml`'s hand-written workflow to `jp`.** Every feature
  provided by `dist` (checksums, attestations, and install scripts) would have
  to be written and maintained in-house.
- **Keeping the `jp_cli` package name.** Avoiding the package rename would
  permanently expose `jp_cli-installer.sh` in user-facing install commands.
- **Building releases with the pinned nightly toolchain.** Regular CI tests run
  on nightly, but `rust-toolchain.toml` specifies stable for releases, and
  shipping binaries built with a nightly compiler is an unnecessary risk for
  users.
- **dist making the Release**, with a description and install instructions of
  its own, and JP's notes written above a marker after it publishes.
  Two workflows would write one description, and the Releases made before dist
  would need a path of their own.
- **The tag's message as the description**, upgrade guide included.
  GitHub gives a Release's description no heading anchors, so the breaking
  entries' links into the guide would go nowhere.

## Non-Goals

- **Command plugins.** A later RFD will distribute the command plugins that are
  ready to ship (`jp-release` among them); `plugins.yml` continues publishing
  its rolling release until then.
- **The macOS app and web client.** Both are prototypes and are not distributed.
- **Notarization and code signing.** macOS Developer ID signing and
  notarization, as well as Windows code signing (all supported by `dist`), are
  deferred to later RFDs.
- **Checksum verification in the install scripts.** `dist`'s shell and
  PowerShell installers do not verify checksums yet (listed as work in progress
  upstream); JP will adopt it once `dist` ships support.
- **Package managers.** Homebrew, npm, MSI installers, and `cargo binstall`
  (which requires publishing JP to crates.io or explicit package metadata) are
  deferred to later work.
- **Self-update.** `dist`'s updater could back a `jp self update` command, which
  belongs in its own RFD.
- **Forge-independent signatures.** Signing releases with tools such as
  minisign, which `cargo binstall --only-signed` verifies, is deferred.

## Risks and Open Questions

- **Stable compiler compatibility.** If JP fails to compile with stable Rust,
  the first release-branch build check will catch the failure so it can be fixed
  before tagging.
- **`dist`'s maintenance.** `dist` has released steadily since 2023, and
  Astral's fork was merged back upstream in 0.29.0.
  If upstream development stalls, `release.yml` continues working at its pinned
  version, and setting `allow-dirty = ["ci"]` lets JP maintain the workflow YAML
  directly.
- **Linux on ARM64 is not tested in CI.** JP's CI runs tests on Linux and
  Windows `x86_64`; the `aarch64-unknown-linux-gnu` binary is compiled by the
  release-branch check, but the test suite is not executed on ARM64 in CI.
- **Minimum supported `glibc`.** Linux binaries require at least the `glibc`
  version shipped by `ubuntu-22.04`.
  Statically linked `musl` builds would support older distributions if needed
  later.

## Implementation Plan

### Phase 1: Release Notes on GitHub

Add `release-notes.yml`, triggered by pushing a release tag and by pushes to
`main` that modify a version's directory: it creates and publishes a GitHub
Release for each tag, writes each description from `main` with building from
source as the install method, and marks the highest version as `latest`.
Depends on [RFD 122] Phases 1, 2, and 4, which render the notes, tag releases,
and publish the upgrade guide on the site.

### Phase 2: Rename and Stable Builds

Rename the `jp_cli` package to `jp`, keeping its library name, and update all
references to the package name (see [Renaming the `jp_cli` Package to
`jp`](#renaming-the-jp-cli-package-to-jp)), including the one-time `cargo
uninstall jp_cli` step in the justfile's `install` recipe.
Add the release toolchain and `just release-build` to the justfile, and run
`just release-build` in CI on pushes to `release/*` for each target.
This phase can merge on its own before the first release cut.

### Phase 3: `dist` and the Release Workflow

Move [RFD 122]'s `release.yml` to `release-train.yml`, then add
`dist-workspace.toml` and the `dist` Cargo profile, generate `release.yml`, pin
its actions, and configure the release toolchain through `github-build-setup`.
`release-notes.yml` stops responding to tag pushes directly: `dist` runs it as a
plan job, where it creates each Release as a draft for `dist` to publish, and as
a post-announce job, and its descriptions switch to the release's install
scripts.
Depends on Phases 1 and 2.

### Phase 4: Installation Guide

Replace the "no binaries" warning in `docs/installation.md` with instructions
for the install scripts, manual archive download and checksum verification,
attestation verification, clearing the macOS quarantine attribute, and building
from source.
Depends on Phase 3.

## References

- [RFD 122: Release Pipeline][RFD 122], which tags releases and renders their
  notes.
- [dist], its [configuration reference][dist-config], [GitHub artifact
  attestations][dist-attestations] in `dist`, and [bringing your own
  release][dist-own-release], which leaves the GitHub Release to JP.
- [GitHub discussion 48311], on Release descriptions having no heading anchors.
- [upload-rust-binary-action] and [create-gh-release-action], the alternative
  built from GitHub Actions.
- [GoReleaser]'s Rust builder.

[GitHub discussion 48311]: https://github.com/orgs/community/discussions/48311
[GoReleaser]: https://goreleaser.com/customization/builds/builders/rust/
[RFD 122]: 122-release-pipeline.md
[create-gh-release-action]: https://github.com/taiki-e/create-gh-release-action
[dist]: https://axodotdev.github.io/cargo-dist/book/
[dist-attestations]: https://axodotdev.github.io/cargo-dist/book/supplychain-security/attestations/github.html
[dist-config]: https://axodotdev.github.io/cargo-dist/book/reference/config.html
[dist-own-release]: https://axodotdev.github.io/cargo-dist/book/ci/customizing.html#bring-your-own-release
[upload-rust-binary-action]: https://github.com/taiki-e/upload-rust-binary-action
