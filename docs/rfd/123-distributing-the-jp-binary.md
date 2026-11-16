# RFD 123: Distributing the jp Binary

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-14
- **Summary**: Builds release binaries for five targets on macOS and publishes
  them to GitHub Releases via `jp release github`.

## Summary

JP's releases ship prebuilt `jp` binaries for Linux, macOS, and Windows.
[RFD 122]'s `jp release build` runs `.config/release/build.sh`, which compiles
all five target triples on macOS using a pinned toolchain and a size-optimized
release profile, producing archives, `checksums.txt`, and `manifest.json`.
A new command plugin, `jp release github`, publishes a verified build to GitHub
Releases: it sets the release description to the rendered release notes, uploads
the archives, checksums, manifest, and install scripts as release assets, and
updates release descriptions when notes change on `main`.
Because building does not depend on GitHub, future publishers (such as hosting
releases directly on `jp.computer`) can consume the same build directory.
Finally, `just backfill` uses these commands to cut, build, and publish
historical releases across JP's commit history in an isolated rehearsal clone
before pushing to GitHub.

## Motivation

Installing JP from source (`cargo install --git`) requires a Rust toolchain and
several minutes of compilation.
[RFD 122] ends a release with a pushed tag and a validated build directory on
the releasing machine; this RFD defines how JP builds its release archives and
publishes them to GitHub.

Although GitHub hosts the repository today, the release build itself should not
depend on GitHub Actions so JP can publish releases to its own website or
migrate to another forge by swapping the publisher plugin.
Binaries are therefore built by `.config/release/build.sh` on the releasing Mac
for now; a remote builder ([RFD 122]) can later move compilation to dedicated
build hardware that attests its output.

JP also has 21 months of commit history prior to its first official release.
Backfilling that history in roughly six-week intervals produces release notes
and prebuilt binaries for each period while exercising the entire release
pipeline in a local clone before anything is pushed to GitHub.

## Design

### What a release ships

| File                                  | Contents                                                                           |
| ------------------------------------- | ---------------------------------------------------------------------------------- |
| `jp-aarch64-apple-darwin.tar.xz`      | `jp` binary, `README.md`, and `LICENSE` for each target                            |
| `jp-x86_64-apple-darwin.tar.xz`       |                                                                                    |
| `jp-x86_64-unknown-linux-gnu.tar.xz`  |                                                                                    |
| `jp-aarch64-unknown-linux-gnu.tar.xz` |                                                                                    |
| `jp-x86_64-pc-windows-msvc.zip`       |                                                                                    |
| `checksums.txt`                       | SHA-256 digests for each archive in `sha256sum` format                             |
| `manifest.json`                       | Version, commit, build profile, and target triple per artifact ([RFD 122])     |
| `install.sh`, `install.ps1`           | Platform install scripts attached by the publisher ([Installation](#installation)) |

Archive filenames omit the version number so download URLs have a predictable
name across releases.
Each archive extracts into a directory of the same stem (e.g.
`jp-aarch64-apple-darwin/`) containing `jp` (or `jp.exe` on Windows) alongside
the `README.md` and `LICENSE` from the built commit (commits before [#77], which
added `LICENSE`, omit it).
Historical releases prior to [#428] (when Windows CI support landed) omit the
Windows archive.

### Building

JP's `build` command runs `.config/release/build.sh` on macOS:

```toml
[plugins.command.release.options]
build = "bash .config/release/build.sh {version} {source} {out} {profile}"
```

The script compiles the `jp` binary (`jp_cli`) from `{source}` for five targets:

- **macOS** (`aarch64-apple-darwin`, `x86_64-apple-darwin`), selecting the first
  Xcode developer directory whose linker supports its bundled macOS SDK (an
  older Command Line Tools linker can fail to parse a newer Xcode SDK; see
  [rust-lang/rust#162928]).
- **Linux** (`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`), using
  [cargo-zigbuild] targeting glibc 2.28 (compatible with Debian 10+, Ubuntu
  20.04+, RHEL 8+, and newer distributions).
- **Windows** (`x86_64-pc-windows-msvc`), using [cargo-xwin] with the CRT
  statically linked (`+crt-static`) so users do not need the Visual C++
  Redistributable installed.

The script reads the target commit's release toolchain from `Cargo.toml`:

```toml
[workspace.metadata.release]
toolchain = "1.99.1"
```

Commits that predate `[workspace.metadata.release]` build with the nightly
toolchain pinned in their `rust-toolchain.toml`.

The `release` profile optimizes for binary size (`opt-level = "s"`, fat LTO,
`codegen-units = 1`, `strip = "symbols"`).
On macOS arm64, this reduces `jp` to 32 MB, compared to 49 MB at `opt-level = 3`
and 88 MB with Cargo's default release profile, while `jp config show`, `jp
conversation ls`, and `jp conversation grep` benchmark within 4% of `opt-level =
3`.
Panics still unwind (`panic = "unwind"`) so a panic inside a spawned task (such
as a tool execution task) is caught and reported as a turn error rather than
aborting the process.
The `quick` profile uses Cargo's default release settings for fast pipeline
testing.

The build script sets profile flags via `CARGO_PROFILE_RELEASE_*` environment
variables, ignores user Cargo config `rustflags`, and remaps local cargo and
rustup paths out of embedded panic strings.
It also sets `SOURCE_DATE_EPOCH` to the commit timestamp so `jp --version`
embeds the commit date rather than the wall-clock build date.

For historical commits using Rust toolchains older than 1.89
([rust-lang/rust#140872]), where `linkme` distributed-slice sections were not
marked `SHF_GNU_RETAIN` on ELF targets, the script disables linker garbage
collection (`--no-gc-sections`) on Linux so Zig's linker does not strip
registered attachment handlers.

### Installation

```sh
# Linux and macOS
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/dcdpr/jp/releases/latest/download/install.sh | sh
```

```powershell
# Windows
powershell -ExecutionPolicy Bypass -c "irm https://github.com/dcdpr/jp/releases/latest/download/install.ps1 | iex"
```

Both scripts detect the OS and CPU architecture, download the matching archive
and `checksums.txt`, verify the archive's SHA-256 checksum, and install `jp`
into `~/.cargo/bin`.
The scripts live in `.config/release/` and contain a `{download}` placeholder
for the release asset base URL (e.g.
`https://github.com/dcdpr/jp/releases/download/v0.3.0`), which the publisher
substitutes before uploading.
Setting `JP_DOWNLOAD` overrides that URL (for example, pointing to a `file://`
build directory to test the installer locally before publishing).
Users can also verify downloaded archives manually with `sha256sum -c
--ignore-missing checksums.txt` or build from source with `cargo install --git
https://github.com/dcdpr/jp.git --tag v0.3.0 jp_cli`.

Because macOS binaries are not notarized, archives downloaded through a web
browser receive the `com.apple.quarantine` extended attribute and are blocked by
Gatekeeper (`curl` and `install.sh` do not set this attribute).
Running `xattr -d com.apple.quarantine jp` removes the attribute.

### Publishing to GitHub

`jp release github` is a standalone command plugin (`jp-release-github`), routed
by longest command prefix ([RFD 072]):

- `jp release github publish 0.3.0` validates the stored `release`-profile build
  for `0.3.0` ([RFD 122]), creates or updates the GitHub Release for `v0.3.0`
  (titled `Release 0.3.0`), uploads the archives, `checksums.txt`,
  `manifest.json`, and any configured `assets` (substituting `{download}` in
  UTF-8 files), and marks the highest published version as `latest`.
  It verifies that tag `v0.3.0` already exists on GitHub at the commit recorded
  in `manifest.json` so creating the release never implicitly creates a tag at
  the tip of `main`.
- `jp release github sync` compares existing GitHub Release descriptions against
  the release notes rendered from the current checkout and updates any that
  differ.
  JP's `.github/workflows/release-notes.yml` runs `sync` on pushes to `main`
  that touch `changelog.d/*/**`.

Because the install scripts and release footer are specific to JP, they are
configured in `.jp/config.toml` rather than hardcoded in the plugin:

```toml
[plugins.command.release-github.options]
assets = [".config/release/install.sh", ".config/release/install.ps1"]
upgrade_guide = "https://jp.computer/releases/{version}/upgrade"
footer = ".config/release/github.md"
```

Each GitHub Release description consists of the release notes rendered from the
current checkout, followed by the contents of `footer` (with `{version}` and
`{download}` substituted).
The notes leave each entry's credits out, as the built site does, since a
Release is read by people deciding whether to upgrade; highlights keep their
bylines ([RFD 122]).
When `upgrade_guide` is configured (as a URL template containing `{version}`),
breaking entries link to that external URL because GitHub Release descriptions
do not generate heading anchors ([GitHub discussion 48311]).
When `upgrade_guide` is unset, the upgrade guide is appended inline after the
release notes.

Because publishing a patch release on an older line would otherwise cause GitHub
to mark that patch as the repository's `latest` release, `publish` explicitly
sets `make_latest` on the highest non-prerelease version across all published
releases.

`publish` authenticates via `JP_GITHUB_TOKEN` or `GITHUB_TOKEN` using
`jp_github`.
New releases are created as drafts, populated with assets, and then undrafted so
a release never appears with missing files; re-publishing an existing release
updates its title, description, and matching assets in place while preserving
any other attached files.

### JP's history

JP's commit history begins in February 2025, with a single test tag (`v0.1.0`)
that was never published as a release.
`just backfill` (`.config/release/backfill.sh`) releases that history in
six-week periods inside an isolated clone whose `origin` points to a local bare
repository, so nothing reaches GitHub until the entire backfill is reviewed:

- `just backfill init` creates a bare mirror of GitHub's `main` (without tags)
  and a working clone whose `origin` points to that bare repository, merges the
  release-tooling branch into the clone's `main`, and assigns a distinct
  workspace ID so backfill conversations and builds stay isolated.
  The clone merges the release-tooling branch via a commit that strips the
  branch's own `changelog.d/` fragments (which describe current tooling rather
  than historical releases), preventing them from appearing during backfill cuts
  or conflicting on subsequent merges.
- `just backfill next` proposes the period's end date, roughly six weeks after
  the previous release (prompting for an optional override), checks out the
  period's last commit in a sibling worktree, and starts drafting.
- `just backfill draft` drafts fragments in an interactive `jp query` session
  using the `release-notes` persona.
  The prompt names the previous release as the baseline for comparison.
  The model groups the period's commits by feature, inspects their diffs and the
  read-only release worktree mounted via `jp query --mount`, and writes one
  fragment per feature via `changelog_add` describing the feature as shipped in
  that release, with `Credits` ([RFD 121]) listing every contributing change.
  If a later commit belongs to an already-drafted feature, the model updates
  that fragment with `fs_modify_file` or removes superseded fragments with
  `fs_delete_file` (each requiring user approval).
  Drafts remain uncommitted until `cut`: `--only` drafts specific changes,
  `--redo` replaces specific fragments, `--fresh` discards the period's
  uncommitted and committed fragments, and `just backfill changes` lists the
  period's changes alongside the fragments that credit them.
- `just backfill cut` commits the drafted fragments to `main`, runs `jp release
  cut --at <commit>` with `GIT_AUTHOR_DATE` and `GIT_COMMITTER_DATE` set to the
  period's end date, runs `jp release build --quick` for the host Mac to verify
  the commit compiles, and checks out the notes branch.
- `just backfill docs` writes the release's user guide inside the release branch
  worktree.
  It carries forward the previous release's user guide via a three-way patch
  apply (seeding the first release's wiki with `main`'s `.epoch` so page IDs do
  not collide with `main`).
  The guide is updated one entry at a time across `added`, `changed`,
  `deprecated`, and `removed` entries, followed by any uncredited changes.
  Each `just backfill docs` invocation starts or resumes an interactive `jp
  query` session with the `release-docs` persona (which has write access to the
  release worktree's wiki via `jp query --mount` and links the fragment to its
  guide page).
  `just backfill docs proceed` commits the entry's guide changes to the release
  branch (dated at the release's end date) and advances to the next entry; `docs
  skip` marks an entry as needing no guide changes, `docs finish` marks all
  remaining entries as done, and `docs <entry>` reopens a specific entry.
- `just backfill highlight <entry>` drafts a release highlight for `<entry>` in
  a conversation that reads the change's diffs, the release's code, and its user
  guide page.
- The maintainer reviews and edits the notes on the notes branch, adds an
  `overview.md`, and previews them with `jp release render` or `just
  develop-docs`.
- `just backfill proceed` verifies that all user guide entries are complete,
  merges the notes branch into `main`, runs `jp release finalize` and `jp
  release build` (full five-target release profile), pushes to the local bare
  `origin`, and starts the next period.
- `just backfill revert 0.5.0` deletes tags `v0.5.0` and later from the bare
  `origin`, runs `jp release abandon` from newest to oldest ([RFD 122]), and
  removes their build directories so `next` can re-cut `0.5.0` while preserving
  any edited fragments.
  Each release's user guide is saved under `refs/backfill/guide/<version>/` and
  restored by `docs` after re-cutting.
- `just backfill publish` finishes the backfill: it deletes the old test tag
  `v0.1.0` on GitHub, pushes all `release/*` branches and `v*` tags to GitHub in
  a single push (pushing more than three tags at once suppresses GitHub Actions
  tag-push workflows for historical commits), runs `jp release github publish`
  for each version from oldest to newest, merges the final release's user guide
  and the branch's own fragments into `main`, and pushes the backfilled
  `changelog.d` history as a branch to open a pull request against `main`.

The user guide commits are the only commits on a release branch not created by
`jp release` itself, and the sole exception to [RFD 122]'s rule that all release
branch commits originate on `main` (because a backfilled release's guide
documents that historical release rather than current `main`).
`jp release finalize` tags the release branch tip including those guide commits,
and `jp release build` builds from that tag.
Each release tag therefore contains the wiki as of its cut point plus the user
guide written for that release, which T-1k4twkf builds and deploys under
`https://jp.computer/vX.Y.Z/wiki/<page>`.

## Drawbacks

- **Release builds require a Mac.** Cross-compiling macOS targets requires the
  macOS SDK and Apple linker, and the build depends on local installations of
  Xcode, Zig, `cargo-zigbuild`, and `cargo-xwin`.
- **Cross-compiled Linux and Windows release archives are not smoke-tested on
  target OSes before upload.** CI tests every commit on Linux and Windows, and
  Linux binaries can be run under Docker on macOS, but Windows archives are not
  executed during `jp release build`.
- **No build provenance attestation.** Checksums verify archive integrity
  against tampering after the build, but do not prove how the binary was built
  until a remote CI builder with artifact attestation is added.
- **Slow full release builds.** Fat LTO with a single codegen unit roughly
  doubles compile time compared to `--quick`.
- **In-tree build and install scripts.** JP maintains
  `.config/release/build.sh`, `install.sh`, and `install.ps1` directly rather
  than delegating to an external release manager.
- **Unsigned binaries.** macOS Gatekeeper blocks archives downloaded via a web
  browser until `com.apple.quarantine` is cleared, and Windows SmartScreen may
  warn on `jp.exe`.
- **Historical commits build against modern cross-linkers.** Backfilled releases
  use their original Rust compiler version alongside current Zig and Windows SDK
  linkers.

## Alternatives

- **[dist] (`cargo-dist`) on GitHub Actions.** Coupling builds to GitHub Actions
  tag-push workflows prevents local release rehearsal, cannot build historical
  commits that predate the `dist` configuration, and names archives after the
  Cargo package (`jp_cli`) unless the package is renamed.
- **[GoReleaser]'s Rust builder.** GoReleaser cross-compiles with
  `cargo-zigbuild`, but defaults to `windows-gnu` rather than `windows-msvc`,
  has incomplete Cargo workspace support, and gates split build/publish
  workflows behind the proprietary [GoReleaser Pro].
- **[upload-rust-binary-action] and [create-gh-release-action].** Both run only
  inside GitHub Actions workflows.
- **Building releases with the pinned nightly toolchain.** CI runs on nightly,
  but official releases should be compiled with a stable Rust toolchain
  (`[workspace.metadata.release].toolchain`).
- **`panic = "abort"`.** Saves binary size, but a panic in any spawned tokio
  task would terminate the entire `jp` process instead of failing only the
  affected tool call.
- **`opt-level = 3` (Cargo's default release optimization level).** Produces a
  53% larger binary on macOS arm64 (49 MB vs 32 MB) and takes twice as long to
  link for at most a 4% speedup in CLI benchmarks.
- **`opt-level = "z"`.** Disables loop vectorization; not benchmarked.
- **Publishing each backfilled release immediately as it is cut.** Because
  published releases cannot be abandoned, any mistake discovered in a later
  period would be irreversible.

## Non-Goals

- **Distributing command plugins.** A future RFD will cover versioned plugin
  distribution (including `jp-release`); `plugins.yml` continues publishing the
  rolling `plugins` release in the meantime.
- **Distributing the macOS app or web client.** Both remain prototypes.
- **Code signing and notarization.** Apple Developer ID notarization and Windows
  Authenticode signing are deferred to future RFDs.
- **Forge-independent cryptographic signatures** (such as `minisign`).
- **Additional publishers.** Publishing to `jp.computer` or object storage can
  be added as separate plugins consuming the same build directory.
- **A remote builder for JP.** [RFD 122] defines the `builders.<name>` hook; JP
  builds locally until dedicated build hardware is set up.
- **Package manager formulas** (Homebrew, Scoop, Winget, `cargo binstall`).
- **Self-update (`jp self update`).**

## Risks and Open Questions

- **Stable toolchain compatibility.** A commit that uses an accidental nightly
  feature or a dependency requiring a newer MSRV would fail at `jp release
  build` time; T-1g695nz adds a CI check with a stable compiler to catch this on
  pull requests.
- **Linux ARM64 test coverage.** CI runs tests on Linux and Windows `x86_64`, so
  `aarch64-unknown-linux-gnu` is only verified by compilation (and optional
  local Docker runs).
- **Historical cut points that fail to compile.** `just backfill cut` runs a
  host `--quick` build immediately after cutting so broken cut points are caught
  before the notes are refined, and `just backfill revert` followed by `next`
  moves the cut point to another day; the drafted fragments keep their credits.
- **Xcode SDK / linker incompatibilities on older toolchains.** Newer macOS SDKs
  can fail to link with older Command Line Tools linkers; `build.sh` probes
  installed developer directories and selects one whose linker and SDK match.
- **Unpublished per-release guide URLs.** Backfilled entries link to guide pages
  under `/vX.Y.Z/`, which do not resolve until T-1k4twkf builds and deploys
  per-release documentation sites.
- **Manual `.order.json` merge when crossing the wiki introduction commit.** The
  wiki landed in `main` on 2026-11-01 (#1264), so the backfill period spanning
  that date already contains `docs/wiki/.order.json`.
  Applying the previous release's guide onto that branch conflicts on
  `docs/wiki/.order.json`; `just backfill docs` pauses so the maintainer can
  merge the two `.order.json` files once.

## Implementation Plan

### Phase 1: Build script

Add `.config/release/build.sh` supporting all five targets,
`[workspace.metadata.release].toolchain`, the `release` and `quick` profiles,
archive packaging, `checksums.txt`, and `manifest.json`.
Configure `plugins.command.release.options.build` and the release toolchain in
`Cargo.toml`, and update `crates/jp_cli/build.rs` to read `SOURCE_DATE_EPOCH`.
Depends on [RFD 122] Phase 6.

### Phase 2: Install scripts

Add `.config/release/install.sh` and `.config/release/install.ps1` with the
`{download}` placeholder and `JP_DOWNLOAD` override, and update
`docs/installation.md` with install script instructions, manual download and
checksum verification, the macOS quarantine note, and building from source.
Depends on Phase 1.

### Phase 3: `jp release github`

Create `crates/plugins/command/release-github` with `publish` (supporting
`assets`, `upgrade_guide`, and `footer`) and `sync`, add the GitHub Releases API
endpoints to `jp_github`, configure `release-github` in `.jp/config.toml`, and
add `.github/workflows/release-notes.yml` to run `sync` when `changelog.d/*/**`
changes on `main`.
Depends on Phases 1 and 2.

### Phase 4: JP's history

Add `.config/release/backfill.sh` (`just backfill`) with `init`, `next`,
`draft`, `changes`, `cut`, `docs`, `highlight`, `proceed`, `revert`, and
`publish`, the `release-notes` and `release-docs` personas with the changelog
and user guide rules, and execute the backfill trial to publish JP's historical
releases.
Depends on Phase 3 and [RFD 122] Phases 5 to 7.

### Repository settings

Before running `just backfill publish`, a repository administrator restricts the
`documentation` GitHub Actions environment to `main` so historical tag pushes
cannot trigger site deployments.
After `publish` replaces the old `v0.1.0` tag, the administrator applies the tag
and branch protection rulesets in T-1f2jmb7.

## References

- [RFD 072: Command Plugin System][RFD 072]: longest-prefix command routing used
  by `jp release github`.
- [RFD 121: Changelog Fragments and Validation][RFD 121]: `Credits` metadata
  used when backfilling historical fragments.
- [RFD 122: Release Pipeline][RFD 122]: `cut`, `pick`, `finalize`, `abandon`,
  and `build`.
- [GitHub discussion 48311]: why GitHub Release descriptions lack heading
  anchors.
- [rust-lang/rust#162928] and [rust-lang/rust#140872]: linker issues handled by
  `.config/release/build.sh`.
- [cargo-zigbuild] and [cargo-xwin]: cross-compilation wrappers for Linux and
  Windows targets.
- [dist], [GoReleaser], [upload-rust-binary-action], and
  [create-gh-release-action]: release tools evaluated in Alternatives.

[#428]: https://github.com/dcdpr/jp/pull/428
[#77]: https://github.com/dcdpr/jp/pull/77
[GitHub discussion 48311]: https://github.com/orgs/community/discussions/48311
[GoReleaser]: https://goreleaser.com/customization/builds/builders/rust/
[GoReleaser Pro]: https://goreleaser.com/pro/
[RFD 072]: 072-command-plugin-system.md
[RFD 121]: 121-changelog-fragments-and-validation.md
[RFD 122]: 122-release-pipeline.md
[cargo-xwin]: https://github.com/rust-cross/cargo-xwin
[cargo-zigbuild]: https://github.com/rust-cross/cargo-zigbuild
[create-gh-release-action]: https://github.com/taiki-e/create-gh-release-action
[dist]: https://axodotdev.github.io/cargo-dist/book/
[rust-lang/rust#140872]: https://github.com/rust-lang/rust/pull/140872
[rust-lang/rust#162928]: https://github.com/rust-lang/rust/issues/162928
[upload-rust-binary-action]: https://github.com/taiki-e/upload-rust-binary-action
