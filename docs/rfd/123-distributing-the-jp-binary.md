# RFD 123: Distributing the jp Binary

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-14
- **Requires**: [RFD 122]
- **Summary**: Release tags publish prebuilt, attested `jp` binaries and install
  scripts to GitHub Releases, built with dist.

## Summary

Every release tag publishes prebuilt `jp` binaries for Linux, macOS, and Windows
on its GitHub Release, with checksums, GitHub artifact attestations, and install
scripts, and with the release's notes as the release's description.
The builds, uploads, and install scripts come from dist (the `cargo-dist`
crate), an existing tool for exactly this, configured to ship the `jp` binary
and nothing else. macOS binaries are not notarized yet, and no plugin or app is
distributed.

## Motivation

The only way to install JP today is `cargo install --git`, which needs a Rust
toolchain and several minutes of compiling.
[RFD 122] cuts a release every two weeks and tags it, but a tag gives a user
nothing to download.
The first official release needs binaries.

Building release binaries for several platforms is a solved problem with
well-used tools, and JP should use one rather than grow its own.
`plugins.yml` already shows what hand-rolling it costs: a matrix of runners,
renaming files, computing checksums with shell, and uploading them one by one.
The `jp` binary deserves the parts a hand-rolled workflow leaves out: checksums
in a standard format, a record of which workflow built each file, and install
scripts that pick the right file for the machine.

The scope is the `jp` binary alone.
It is the most urgent artifact and hard enough to get right on its own; the
command plugins follow in a later RFD, and the macOS app and web client are
prototypes that are not distributed.

## Design

### What a release publishes

When a person pushes a release tag (`jp release finalize`, [RFD 122]), the
GitHub Release for that tag gets:

| Asset                                 | What it is                                                   |
| ------------------------------------- | ------------------------------------------------------------ |
| `jp-x86_64-unknown-linux-gnu.tar.xz`  | The `jp` binary for each target, with the README and license |
| `jp-aarch64-unknown-linux-gnu.tar.xz` |                                                              |
| `jp-aarch64-apple-darwin.tar.xz`      |                                                              |
| `jp-x86_64-apple-darwin.tar.xz`       |                                                              |
| `jp-x86_64-pc-windows-msvc.zip`       |                                                              |
| `*.sha256`, `sha256.sum`              | A SHA-256 checksum per archive, and all of them in one file  |
| `jp-installer.sh`, `jp-installer.ps1` | Install scripts for Unix shells and PowerShell               |
| `dist-manifest.json`                  | A machine-readable list of the assets                        |

The archive names carry no version, so
`releases/latest/download/jp-installer.sh` always fetches the newest release.
Each archive also gets a GitHub artifact attestation: a Sigstore-signed record
of the workflow run and commit that built it, kept by GitHub rather than
uploaded as an asset.

The release's description is its release notes, rendered from the release's
directory on `main` ([Release notes](#release-notes)), above the install
instructions.

The targets are the four `plugins.yml` already builds, plus Linux on ARM64,
which covers ARM servers and Linux containers on Apple Silicon.

### Installing

```sh
# Linux and macOS
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/dcdpr/jp/releases/latest/download/jp-installer.sh | sh
```

```powershell
# Windows
powershell -ExecutionPolicy Bypass -c "irm https://github.com/dcdpr/jp/releases/latest/download/jp-installer.ps1 | iex"
```

The scripts detect the platform, download the matching archive, and install `jp`
into Cargo's bin directory (`~/.cargo/bin`), the same place `cargo install` puts
it.
Downloading an archive by hand works too: `sha256sum -c sha256.sum` checks it,
and `gh attestation verify` checks which workflow built it, for those who use
GitHub's CLI.
`cargo install --git` stays as the way to build from source.

macOS binaries are not notarized. macOS checks notarization only for files
marked as quarantined, which browsers do and `curl` and the install script do
not, so only a binary downloaded through a browser is blocked; `xattr -d
com.apple.quarantine jp` lets it run.
The installation guide says so, and a later RFD adds Developer ID signing and
notarization.

### The tool: dist

[dist] (the `cargo-dist` crate) builds a Cargo workspace's binaries on GitHub's
runners and publishes them to a GitHub Release.
JP configures it in `dist-workspace.toml`:

```toml
[workspace]
members = ["cargo:."]

[dist]
cargo-dist-version = "0.33.0"
ci = ["github"]
packages = ["jp"]
display-name = "JP CLI"
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
post-announce-jobs = ["./release-notes"]
github-build-setup = "build-setup.yml" # steps that select the release toolchain

[dist.github-custom-runners]
aarch64-unknown-linux-gnu = "ubuntu-22.04-arm"

[dist.github-action-commits]
# Every action pinned to a commit, as JP's other workflows are.
```

- `packages = ["jp"]` makes the `jp` package the only one dist builds; every
  other crate, plugins included, is ignored, whatever its `publish` setting.
- `display-name = "JP CLI"` names the package in the install section dist writes
  on the release page ("Install JP CLI 0.3.0"); file names stay `jp-…`.
- `precise-builds = true` builds the package alone (`cargo build -p jp`), as
  `cargo install` does, instead of dist's default of building the whole
  workspace so features unify across it.
- `source-tarball = false` leaves out dist's `source.tar.gz`; GitHub attaches
  the source code to every release on its own, and the list of assets stays the
  binaries and the files that install or check them.
- `pr-run-mode = "skip"` keeps dist off pull requests; the release-branch check
  below covers what it would.
- `github-action-commits` pins every action dist's workflow uses to a commit.

dist owns the workflow it generates, `.github/workflows/release.yml`.
`dist generate` writes it from the configuration, and `dist plan` fails while it
is out of date, so the configuration is the source of truth and the YAML is
never edited by hand.
It runs on pushes of version tags: dist builds the packages whose version the
tag names, which is `jp` because [RFD 122]'s `bump` sets its version on the
release branch, builds each target on a native runner, attests and uploads the
archives, and publishes the GitHub Release.

The tag push is a person's, so the workflow starts on its own, without approval
([RFD 122]'s JP's CI).

### The package is renamed `jp`

dist names archives and install scripts after the Cargo package, and the package
that builds `jp` is `jp_cli`, which would publish `jp_cli-installer.sh`.
Once published, those names are in install commands and scripts users copy, so
the first release fixes them for good.
The package is renamed `jp` before it, and `[lib] name = "jp_cli"` keeps the
library's name.
The rename then changes only the places that name the package:

- `crates/jp_cli/Cargo.toml` and its entry in `Cargo.lock`.
- `--package jp_cli` in the justfile's `run` and `serve-web` recipes.
- The `debug_jp` tools in `.config/jp/tools`, which build with `-p jp_cli`.
- The `cargo install` command in `README.md` and `docs/installation.md`.
- The example in the `cargo_check` tool's description.
- [RFD 122]'s `bump` command, which becomes `--package jp`.

Everything that names the library or the directory stays as it is: the
`crates/jp_cli/` directory, `jp_cli::` paths, tracing targets and `RUST_LOG`
filters, insta snapshot names, and the paths in personas and tool docs.
The `jp` binary and its `--help` and `--version` output do not change either;
clap's command name is set to `jp` explicitly, not taken from the package.
The ticket label `package=jp_cli` keeps its value: it names the area of code
under `crates/jp_cli`, and nothing checks it against Cargo.

One step is needed on every machine that installed `jp` with `cargo install`,
which includes every contributor through `just install`.
Cargo records that the `jp_cli` package owns `~/.cargo/bin/jp`, so installing
the `jp` package refuses to overwrite it until `jp_cli` is uninstalled.
The justfile's install step runs `cargo uninstall jp_cli` while Cargo still
lists it, and the installation guide tells users who built from source to do the
same.

### The compiler

`rust-toolchain.toml` pins a nightly for development and says release binaries
are built with stable.
The release workflow sets `RUSTUP_TOOLCHAIN` to a stable version through dist's
`github-build-setup` steps, and that version lives in one place, the justfile,
next to the other tool versions it pins.
Nothing builds JP with stable today, so the first release would be the first to
find out whether it does; the release-branch check makes it the first cut
instead.

### The release-branch check

`just release-build` runs `dist build` for the host target with the release
toolchain: the same tool, configuration, and compiler as the tag's build, on one
machine.
JP's CI runs it on every push to a `release/*` branch, on each target's runner,
so a release branch that does not build or package for some platform shows red
before anyone tags it, and [RFD 122]'s finalize waits on the branch's checks.
It also runs locally, for anyone checking a build without GitHub.

### Release notes

The GitHub Release's description is the release notes [RFD 122] renders from
`changelog.d/<version>/` on `main`, followed by a link to the upgrade guide when
there is one, and then dist's install instructions.

A reusable workflow, `release-notes.yml`, writes it. dist calls it after it
publishes the release (`post-announce-jobs`), and it also runs on every push to
`main` that changes a version's directory, rewriting the description of each
affected release that exists, so a correction to an old release's notes reaches
its GitHub Release.
It renders with the `changelog` binary and the forge and headings from the
settings snapshot, as the docs build does ([RFD 122]'s On the site), so it
compiles one small crate rather than `jp`.
It keeps the install instructions dist wrote below a marker and replaces
everything above it.

The same workflow marks the release with the highest version as GitHub's latest.
GitHub always has a latest release, which `releases/latest/download/` serves.
dist publishes with `gh release create` and no `--latest` flag, so `gh` sends no
`make_latest`, and GitHub's API documents the default as making each newly
published release the latest.
A patch release to an older line can publish after a newer release ([RFD 122]'s
Releases in parallel); left alone, 0.3.1 would become the latest after 0.4.0,
and the install command would install it.
`gh` describes the same default as "automatic based on date and version", which
would make the step unnecessary; the step is correct under either reading, so
the disagreement does not need settling.

A release tagged before its cut's change request merges has no directory on
`main` yet; its description says the notes are coming, with a link to the
release page on the site, until that change request merges and the workflow
fills them in.

## Drawbacks

- **A tool owns a workflow.** `release.yml` is generated, not written, and does
  not follow JP's convention of running `just` recipes; its steps call dist
  directly.
  The release-branch check is the `just` recipe that reproduces its build
  locally.
- **Another tool to keep current.** `cargo-dist-version` pins dist, and
  upgrading it means regenerating `release.yml` and reviewing the diff.
- **Experimental features.** GitHub attestations and `github-build-setup` are
  marked experimental in dist.
- **Verifying an attestation needs GitHub's CLI.** Checksums need nothing but
  `sha256sum`; signatures that verify without GitHub are later work.
- **The package rename** touches the handful of places that name the package,
  and asks everyone who installed with `cargo install` to uninstall `jp_cli`
  once.
- **Unsigned binaries.** Windows may warn about an unsigned binary downloaded
  through a browser, and macOS blocks a browser-downloaded binary until its
  quarantine attribute is removed.

## Alternatives

- **[upload-rust-binary-action] with [create-gh-release-action].** Small,
  well-maintained GitHub Actions that build one binary per matrix job and upload
  it with checksums.
  JP would write and maintain the workflow around them: the runner matrix,
  attestations, release notes, and any install script.
  They run only inside GitHub Actions.
  This is the better choice if dist's generated workflow becomes a burden.
- **[GoReleaser]'s Rust builder.** One command that builds every target from one
  machine and runs locally.
  It cross-compiles with `cargo-zigbuild`, which is least reliable for macOS and
  Windows MSVC, the targets where JP's dependencies reach into the platform, and
  JP's CI already builds natively on all three.
  It also brings a Go tool into a Rust project's release.
- **Extending `plugins.yml`'s hand-written workflow to `jp`.** Every feature
  above would be ours to write and keep working.
- **Keeping the `jp_cli` package name.** No rename, and install commands that
  say `jp_cli-installer.sh` for good.
- **Building releases with the pinned nightly.** It is what CI tests, but
  `rust-toolchain.toml` says releases use stable, and a nightly is a poor thing
  to hand users.

## Non-Goals

- **Command plugins.** A later RFD distributes the plugins that are ready,
  `jp-release` among them; `plugins.yml` keeps publishing its rolling release
  until then.
- **The macOS app and the web client.** Prototypes, not distributed.
- **Notarization and code signing.** macOS Developer ID signing and
  notarization, and Windows code signing, which dist supports, come in later
  RFDs.
- **Checksum checks in the install scripts.** dist's shell and PowerShell
  installers do not verify checksums yet, which dist lists as work in progress;
  JP picks it up when dist ships it.
- **Package managers.** Homebrew, npm, MSI, and `cargo binstall`, which needs JP
  on crates.io or explicit metadata, are later work.
- **Self-update.** dist's updater could back a `jp self update`, which is its
  own RFD.
- **Signatures that verify without GitHub**, such as minisign, which `cargo
  binstall --only-signed` checks.

## Risks and Open Questions

- **Stable builds.** If JP does not build with stable, the first cut finds out
  and the fix comes first.
- **dist's maintenance.** dist has released steadily since 2023, and Astral's
  fork of it was merged back in 0.29.0.
  If it stalls, `release.yml` keeps working at its pinned version, and
  `allow-dirty = ["ci"]` hands the YAML to JP to maintain by hand.
- **Linux on ARM64 is not tested.** JP's CI tests on Linux and Windows x86_64;
  the ARM64 build is compiled by the release-branch check but never run in CI.
- **The oldest supported glibc** is whatever `ubuntu-22.04` ships.
  Statically linked musl builds would run on older systems; nothing asks for
  them yet.

## Implementation Plan

### Phase 1: Rename and stable builds

Rename the `jp_cli` package to `jp`, keeping its library name, and update the
places that name the package ([The package is renamed
`jp`](#the-package-is-renamed-jp)), including the one-time `cargo uninstall
jp_cli` in the justfile's install step.
Add the release toolchain to the justfile and `just release-build`, and run it
in CI on pushes to `release/*` for each target.
Mergeable on its own, and worth landing before the first cut.

### Phase 2: dist and the release workflow

Add `dist-workspace.toml` and the `dist` profile, generate `release.yml`, pin
its actions, and set the toolchain through `github-build-setup`.
Depends on Phase 1 and on [RFD 122] Phase 2, which tags releases.

### Phase 3: Release notes

Add `release-notes.yml`, called after each release is published and on pushes to
`main` that change a version's directory, which writes the description and marks
the highest version as the latest release.
Depends on Phase 2 and on [RFD 122] Phase 1, which renders release notes.

### Phase 4: Installation guide

Replace the "no binaries" warning in `docs/installation.md` with the install
scripts, manual download and checksum verification, attestation verification,
the macOS quarantine note, and installing from source.
Depends on Phase 2.

## References

- [RFD 122: Release Pipeline][RFD 122], which tags releases and renders their
  notes.
- [dist], its [configuration reference][dist-config], and [GitHub artifact
  attestations][dist-attestations] in dist.
- [upload-rust-binary-action] and [create-gh-release-action], the alternative
  built from GitHub Actions.
- [GoReleaser]'s Rust builder.

[GoReleaser]: https://goreleaser.com/customization/builds/builders/rust/
[RFD 122]: 122-release-pipeline.md
[create-gh-release-action]: https://github.com/taiki-e/create-gh-release-action
[dist]: https://axodotdev.github.io/cargo-dist/book/
[dist-attestations]: https://axodotdev.github.io/cargo-dist/book/supplychain-security/attestations/github.html
[dist-config]: https://axodotdev.github.io/cargo-dist/book/reference/config.html
[upload-rust-binary-action]: https://github.com/taiki-e/upload-rust-binary-action
