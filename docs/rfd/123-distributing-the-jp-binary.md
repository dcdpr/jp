# RFD 123: Distributing the jp Binary

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-14
- **Requires**: [RFD 122]
- **Summary**: JP's script builds release binaries for five targets on the
  releasing Mac, and `jp release github` publishes them with their notes.

## Summary

JP's releases ship prebuilt `jp` binaries for Linux, macOS, and Windows.
[RFD 122]'s `jp release build` runs JP's build script, which builds all five
targets on one Mac with a pinned toolchain and a profile tuned for small
binaries, and writes their archives, their checksums, and a manifest.
A new plugin, `jp release github`, publishes a built release to GitHub: its
notes as the Release's description, its archives and checksums as assets, and
install scripts that verify them.
Building depends on no forge, so publishing somewhere else, such as on JP's own
site, is a publisher that reads the same build.
The same commands release JP's history, from its first commit up to its first
regular release, in a trial run that publishes nothing until it ends.

## Motivation

The only way to install JP today is `cargo install --git`, which needs a Rust
toolchain and several minutes of compiling.
[RFD 122] ends a release with a pushed tag and a build on the releasing machine;
this RFD decides what that build contains and where it goes.

JP is moving away from depending on GitHub.
GitHub hosts the repository and will publish releases, but releasing has to work
with git alone, so that JP can publish binaries on its own site, or move to
another forge, by changing a publisher rather than its release process.
Binaries are therefore built by JP's own script, on the releasing machine for
now; a remote builder ([RFD 122]) can later move builds to dedicated, vetted
hardware that a CI job drives and that attests what it builds.

JP has 21 months of history and no releases.
Releasing that history in six-week periods gives each its notes and binaries,
and doing it with the commands a release uses tests those commands, for weeks,
before anything is published.

## Design

### What a release ships

| File                                  | What it is                                                                          |
| ------------------------------------- | ----------------------------------------------------------------------------------- |
| `jp-aarch64-apple-darwin.tar.xz`      | The `jp` binary for each target, with the README and license                        |
| `jp-x86_64-apple-darwin.tar.xz`       |                                                                                     |
| `jp-x86_64-unknown-linux-gnu.tar.xz`  |                                                                                     |
| `jp-aarch64-unknown-linux-gnu.tar.xz` |                                                                                     |
| `jp-x86_64-pc-windows-msvc.zip`       |                                                                                     |
| `checksums.txt`                       | A SHA-256 for each archive, in `sha256sum`'s format                                 |
| `manifest.json`                       | The version, commit, and profile built, and each archive's platform ([RFD 122]) |
| `install.sh`, `install.ps1`           | Install scripts, which JP's publishers add ([Installation](#installation))          |

The archive names carry no version, so a publisher can serve the newest
release's files at one address.
Each archive holds a directory named after it, such as
`jp-aarch64-apple-darwin/`, with `jp`, or `jp.exe`, and the `README.md` and
`LICENSE` of the commit built; commits from before [#77], which added the
license, have none to ship.
Releases from before [#428], which added Windows to CI, have no Windows archive:
no Windows build of them was ever run.

### Building

JP's `build` command is a script that runs on macOS:

```toml
[plugins.command.release.options]
build = "bash .config/release/build.sh {version} {source} {out} {profile}"
```

It builds the `jp` binary of the `jp_cli` package from `{source}` for each
target:

- **macOS**, both architectures, with the clang, linker, and SDK of the first
  developer directory whose linker reads its own SDK, since an older linker
  can't read a newer SDK ([rust-lang/rust#162928]).
- **Linux**, both architectures, with [cargo-zigbuild] against glibc 2.28, so
  the binaries run on Debian 10, RHEL 8, and anything newer.
- **Windows** with [cargo-xwin], with the C runtime linked in, so it needs no
  Visual C++ runtime installed.

The toolchain is the commit's release toolchain, an exact stable version in
`Cargo.toml`:

```toml
[workspace.metadata.release]
toolchain = "1.99.1"
```

A commit without one, which is every commit from before this RFD's first phase,
builds with the nightly its `rust-toolchain.toml` pins, the one its CI tested.

The `release` profile makes the binaries as small as the compiler can without
changing how they behave: it optimizes for size (`opt-level = "s"`), with fat
link-time optimization, one codegen unit, and symbols stripped.
On macOS arm64 that makes `jp` 32 MB, against 49 MB at the default optimization
level and 88 MB with Cargo's default release profile, and `jp config show`, `jp
conversation ls`, and `jp conversation grep` ran at most 4% faster at the
default level.
Panics still unwind: a panic in a spawned task, such as a tool call's, reaches
the turn as an error rather than ending `jp`.
The `quick` profile is Cargo's default release profile, for testing the script
in a fraction of the time.
The script sets both through `CARGO_PROFILE_RELEASE_*` and ignores rustflags in
Cargo's configuration files, so every commit builds the same way, whatever its
`Cargo.toml` or the releaser's settings say, and it remaps the releaser's paths,
such as Cargo's registry, out of the panic messages the binaries carry.
It sets `SOURCE_DATE_EPOCH` to the commit's time, which `jp --version` shows as
its build date, so a release reports the day it was cut rather than the day it
was built; commits from before this RFD's first phase show the day they were
built.

Compilers older than Rust 1.89 ([rust-lang/rust#140872]) don't mark `linkme`'s
registrations as kept on Linux, and Zig's linker discards them, so those
compilers build Linux targets without the linker's dead-code removal.

### Installation

```sh
# Linux and macOS
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/dcdpr/jp/releases/latest/download/install.sh | sh
```

```powershell
# Windows
powershell -ExecutionPolicy Bypass -c "irm https://github.com/dcdpr/jp/releases/latest/download/install.ps1 | iex"
```

The scripts detect the platform, download its archive and `checksums.txt`,
refuse an archive whose checksum doesn't match, and install `jp` into Cargo's
bin directory (`~/.cargo/bin`), where `cargo install` puts it.
They are JP's files, in `.config/release/` next to its build script, and hold a
`{download}` placeholder for the address a release's files are served from, such
as `https://github.com/dcdpr/jp/releases/download/v0.3.0`, which the publisher
fills in, so the same scripts work for any publisher.
`JP_DOWNLOAD` names another address, such as `file://` and a build's directory,
to try a script before anything is published.
An archive downloaded by hand is checked with `sha256sum -c --ignore-missing
checksums.txt`, and `cargo install --git` with `--tag` builds a release from
source.

macOS binaries are not notarized. macOS checks notarization only for files a
browser marks as quarantined, which `curl` and the install script do not, so
only a binary downloaded through a browser is blocked; `xattr -d
com.apple.quarantine jp` lets it run.

### Publishing to GitHub

`jp release github` is a plugin of its own, `jp-release-github`, which the host
runs for that command because the longest command path wins ([RFD 072]).
A workspace that doesn't install it publishes nothing to GitHub but its tags.

- `jp release github publish 0.3.0` publishes a release's publishable build
  ([RFD 122]): it creates or updates the GitHub Release of `v0.3.0`, titled
  `Release 0.3.0`, uploads the archives, `checksums.txt`, `manifest.json`, and
  the files its `assets` setting names, and marks the release with the highest
  version as the latest.
  Each of those files is uploaded with every `{download}` in it replaced by the
  address GitHub serves the release's files from.
- `jp release github sync` rewrites the description of each Release whose notes
  changed on `main`, and JP's CI runs it on pushes to `main` that change a
  version's directory.

The install scripts install `jp` alone, so they are JP's to list, and a
workspace releasing something else lists its own files or none:

```toml
[plugins.command.release-github.options]
assets = [".config/release/install.sh", ".config/release/install.ps1"]
```

A Release's description is the release's notes, rendered from `main` as the site
renders them, followed by the commands that install that release.
Breaking entries link to the upgrade guide on JP's site rather than include it,
as the tag's message does: GitHub gives a description no heading anchors, so
links to its own sections would go nowhere ([GitHub discussion 48311]).
The description follows `main`, while the tag keeps the notes the release was
tagged with.

A patch release to an older line can be published after a newer release, and
GitHub makes a newly published Release the latest unless told otherwise, so
`publish` marks the highest version as the latest each time; the latest is what
`releases/latest/download/` serves.

`publish` reads its token from `JP_GITHUB_TOKEN` or `GITHUB_TOKEN` and talks to
GitHub's API through `jp_github`.
GitHub's immutable releases stay off.
With them on, `publish` would upload to a draft before publishing it, since an
immutable Release takes no assets afterwards, and artifact attestations would
come from a remote builder on GitHub Actions.

### JP's history

JP's history starts in February 2025, and its one tag, `v0.1.0`, was a test that
nothing was published for.
That history is released after the fact, in periods of about six weeks, with the
commands any release uses, in a clone where nothing reaches GitHub until the
trial ends:

- `just backfill init` clones the repository and points the clone's `origin` at
  a bare repository standing in for GitHub, which holds a copy of GitHub's
  `main` and no tags, and removes every other remote.
- `just backfill next` prints the next period's suggested last day, six weeks
  after the previous release's, and reads a line in which another day can be
  typed.
  It drafts fragments for that period's changes from git history, with `Credits`
  naming their pull requests ([RFD 121]), in a local conversation with the
  `prose` persona, and commits them to `main`.
  It then runs `jp release cut --at` on the period's last commit with git's
  dates set to that day, builds the release for this Mac with `--quick`, so a
  period that doesn't build is caught before its notes are written, and checks
  out the notes branch.
- The notes are refined like any release's, and read with `jp release render` or
  as draft pages under `just develop-docs`.
- `just backfill proceed` merges the notes branch into `main`, runs `jp release
  finalize` and `jp release build`, runs the pushes they print, and starts the
  next period.
- `just backfill revert 0.5.0` deletes the stand-in's tags of 0.5.0 and every
  later release and abandons them, newest first ([RFD 122]), so the next `next`
  cuts 0.5.0 again with its notes as they were edited.
- `just backfill publish` ends the trial.

The stand-in starts without tags, so the first period is release 0.1.0, and
`next` suggests the commit the old `v0.1.0` named as its end.
Every period builds with its commit's toolchain and, before [#428], without
Windows ([Building](#building)).
Drafting skips changes that already have a fragment, so the last period, which
ends at `main`'s tip, keeps the fragments written since the gate landed as they
were written, and becomes JP's first regular release.

`publish` deletes GitHub's old `v0.1.0`, pushes every release branch and tag in
one push, and runs `jp release github publish` for each release, oldest first.
Last, it pushes the stand-in's `main`, merged with GitHub's, as a branch whose
pull request brings the history's notes into GitHub's `main`; the tags are
already there when that merges, so the site gets every release's pages at once.
GitHub starts no workflows for a push of more than three tags, so the history's
commits, whose workflows deploy the site on any tag, run nothing.
Anyone who fetched the old `v0.1.0` deletes it with `git tag -d v0.1.0`, since
`git fetch` refuses to replace a tag it already has.

## Drawbacks

- **Releases are built on a Mac.** macOS targets need Apple's SDK, so the
  releasing machine is a Mac, and a release depends on the Xcode, Zig,
  cargo-zigbuild, and cargo-xwin installed there.
- **Linux and Windows binaries are cross-compiled and not run before they
  ship.** JP's CI tests on Linux and Windows, but not the release binaries;
  Docker can run the Linux ones on the releasing Mac, and nothing runs the
  Windows one.
- **No attestation.** A local build's checksums show that a file is the one
  published, not how it was built, so trusting a release means trusting the
  releasing machine until a remote builder attests its builds.
- **Release builds are slow.** Fat link-time optimization with one codegen unit
  takes about twice as long as a quick build, which is why testing uses
  `--quick`.
- **JP maintains its build and install scripts**, where a release tool would.
- **Unsigned binaries.** Windows may warn about an unsigned binary downloaded
  through a browser, and macOS blocks one until its quarantine attribute is
  removed.
- **Old commits built with today's tools.** The history's releases build with
  the compiler their CI used but today's linkers, and a commit whose git
  dependencies have since vanished can't be built at all.

## Alternatives

- **[dist] on GitHub Actions**, this RFD's earlier design. dist builds on
  GitHub's runners when a tag is pushed and publishes to a GitHub Release, so a
  release could not be built or rehearsed without GitHub, and commits from
  before its configuration could not be built with it.
  It also names archives after the Cargo package, which would have meant
  renaming `jp_cli` to `jp`; JP's script names them itself, so the package keeps
  its name.
- **[GoReleaser]'s Rust builder.** It cross-compiles with cargo-zigbuild too,
  but defaults to Windows' GNU target, where JP ships and tests MSVC, and
  documents Cargo workspaces as possibly not working.
  Building now and publishing later, merging builds from several machines, and
  importing binaries built elsewhere are in [GoReleaser Pro], which is paid and
  closed source.
- **[upload-rust-binary-action] and [create-gh-release-action].** They build on
  GitHub's runners and upload to a Release, inside GitHub Actions only.
- **Building new releases with the pinned nightly**, as the history's are.
  It is what CI tests, but a nightly is a poor thing to hand users, and
  `rust-toolchain.toml` says releases build with stable.
- **`panic = "abort"`**, which makes the binaries smaller but ends `jp` on a
  panic in any spawned task, instead of failing one tool call.
- **The default optimization level** (`opt-level = 3`), which makes `jp` 53%
  larger on macOS arm64 and takes twice as long to build, for at most 4% more
  speed in the commands measured.
- **`opt-level = "z"`**, which also turns off loop vectorization; its size and
  speed are not measured.
- **Publishing each of the history's releases as it is cut.** A published
  release can't be taken back, so the trial would end at its first mistake.

## Non-Goals

- **Command plugins.** A later RFD will distribute the command plugins that are
  ready to ship (`jp-release` among them); `plugins.yml` continues publishing
  its rolling release until then.
- **The macOS app and web client.** Both are prototypes and are not distributed.
- **Notarization and code signing.** macOS Developer ID signing and
  notarization, and Windows code signing, come in later RFDs.
- **Signatures that verify without GitHub**, such as minisign.
- **Other publishers.** JP's site, with binaries on storage it links to, can
  publish releases from the same builds in a later RFD.
- **A remote builder for JP.** [RFD 122] defines how one plugs in; JP builds on
  the releasing Mac until it has dedicated hardware.
- **Package managers**, such as Homebrew, Scoop, Winget, and `cargo binstall`.
- **Self-update.**

## Risks and Open Questions

- **Stable builds.** A commit that doesn't build with its release toolchain
  fails its release build, after its tag; T-1g695nz checks a stable compiler in
  CI, which catches that before a cut.
- **Linux on ARM64 is not tested.** JP's CI tests on Linux and Windows x86_64;
  the ARM64 binary runs only in Docker on the releasing Mac.
- **A period whose last commit doesn't build.** `next` builds for this Mac right
  after each cut, so such a period is caught before its notes are written, and
  `revert` with another day ends it at a commit that builds.
- **Apple's tools change under old commits.** A new Xcode can stop old commits
  from linking, as macOS 27's SDK did with older linkers; the script picks a
  developer directory that links, and names the one it used.

## Implementation Plan

### Phase 1: Build script

Turn the build spike into `.config/release/build.sh`: the five targets, the
release toolchain from `[workspace.metadata.release]`, the `release` and `quick`
profiles, the archives, `checksums.txt`, and `manifest.json`.
Set JP's `build` and release toolchain, and make `jp --version` read
`SOURCE_DATE_EPOCH`.
Depends on [RFD 122] Phase 6.

### Phase 2: Install scripts

Add `install.sh` and `install.ps1` to `.config/release/`, with the `{download}`
placeholder and `JP_DOWNLOAD`, and replace the "no binaries" warning in
`docs/installation.md` with them, manual download and checksum verification, the
macOS quarantine note, and building from source.
Depends on Phase 1.

### Phase 3: `jp release github`

Create the plugin with `publish`, its `assets` setting, and `sync`, add GitHub's
Releases API to `jp_github`, set JP's `assets` to its install scripts, and run
`sync` in JP's CI on pushes to `main` that change a version's directory.
Depends on Phases 1 and 2.

### Phase 4: JP's history

Add `just backfill` with `init`, `next`, `proceed`, `revert`, and `publish`, and
run the trial.
Its `publish` is JP's first release with binaries.
Depends on Phase 3 and on [RFD 122] Phases 5 to 7.

### Repository settings

Before `just backfill publish`, someone with admin access to the repository
restricts the `documentation` environment to deployments from `main`, which
keeps any stray tag from deploying the site.
After it, they apply T-1f2jmb7's rulesets, which block deleting tags, something
`publish` does once, to `v0.1.0`.

## References

- [RFD 072: Command Plugin System][RFD 072], whose longest command path routes
  `jp release github` to its own plugin.
- [RFD 121: Changelog Fragments and Validation][RFD 121], the `Credits` that
  file the history's fragments.
- [RFD 122: Release Pipeline][RFD 122], which cuts, tags, and builds releases.
- [GitHub discussion 48311], on Release descriptions having no heading anchors.
- [rust-lang/rust#162928] and [rust-lang/rust#140872], the linker problems the
  build script works around.
- [cargo-zigbuild] and [cargo-xwin], which cross-compile the Linux and Windows
  targets.
- [dist], [GoReleaser], [upload-rust-binary-action], and
  [create-gh-release-action], the tools considered.

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
