# RFD 122: Release Pipeline

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-01
- **Extends**: [RFD 116]
- **Required by**: [RFD 123]
- **Summary**: The `jp release` plugin cuts, tags, and builds releases on the
  releasing machine, keeping their notes on `main`, for the person releasing to
  publish.

## Summary

This RFD extends the `jp release` plugin ([RFD 121]) with commands that cut,
patch, tag, and build releases while keeping all release notes and upgrade
guides on `main`.
`jp release cut` branches `release/<series>` for code and
`release-notes/v<version>` for notes, which moves the unreleased fragments into
the release's directory once it is merged into `main`; `jp release pick`
cherry-picks fixes onto the release branch, `jp release finalize` tags the
release with its notes as the tag's message, and `jp release build` runs the
project's build command for it.
Each command works on the releasing machine, talks to no forge, and prints the
`git push` that publishes what it made, so a release can be rehearsed, or undone
with `jp release abandon`, without publishing anything.
It replaces `jilu`; what JP builds and where it publishes binaries are covered
in [RFD 123].

## Motivation

Cutting a release should be automated and routine.
JP starts with a two-week release cadence while development moves fast and
releases mainly serve contributors.
Because the cadence is configurable, it can slow down once external users depend
on stable releases.

[RFD 121] collects per-change fragments and per-feature highlights in each pull
request; this RFD assembles them into published releases.
Following Bevy's model, each release produces two documents: release notes for
an overview of changes, and an upgrade guide for breaking changes.
Separating migration steps keeps the release notes focused without truncating
upgrade instructions.

Editing release notes can take days, and development on `main` should not wait
for it.
Cutting a release onto its own branch freezes the code in that release while
`main` continues advancing.
Any additional fixes are cherry-picked onto the release branch explicitly.

Each artifact has a single location and each step has a single command.
Release notes live only on `main`, and release branches delete `changelog.d/` on
creation, so note corrections always happen on `main`.
Updates to `main` go through standard change requests, requiring no
branch-protection exceptions.

Like the plugins in [RFD 116], `jp release` works in any JP workspace and does
not require a forge web interface.
Every release step is a CLI command; a forge is used only to review change
requests, and projects without a forge can run the entire workflow with Git
alone.
JP's CI workflows wrap these commands as a convenience rather than a
requirement.

The commands publish nothing themselves.
What they make stays in the local repository until the person releasing pushes
it, so a mistake is undone by deleting a branch, and a release can be rehearsed
on a branch of its own before the real one.

Binaries follow the same rule.
A release ends on the releasing machine, with its tag pushed and its binaries
built; putting the binaries on a forge, a website, or object storage is a step
of its own, which another plugin takes ([RFD 123]).
Building runs a command the project configures, so it can move to dedicated
build hardware without changing what reads its results.

Releases can also be cut at an earlier commit of `main`, so a project can
release the history it had before it had releases.

## Design

### Repository layout

| Where                                                  | Holds                                                                                                                                      |
| ------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------ |
| `main`                                                 | Development, and every release's notes: unreleased fragments at the top level of `changelog.d/`, each release's in its version's directory |
| `release/0.3`                                          | The code 0.3.x releases ship: `main` at the cut, without `changelog.d/`, plus the commits picked onto it                                   |
| `release-notes/v0.3.0`                                 | A release's notes on their way to `main`: its fragments moved into its directory, to be merged into `main`                                 |
| `v0.3.0`, `v0.3.1`                                     | Tags on `release/0.3`: the releases                                                                                                        |
| `release/0.3.0/` in the workspace's user-local storage | A release's build: its archives, their checksums, and a manifest ([Building a release](#building-a-release))                               |

No separate state file is used:

- A release **ships** a change when its branch contains the commit that made it,
  from `main`'s history at the cut or through a pick.
- A fragment in a version's directory belongs to that release.
  One at the top level is **unreleased** until a release ships its change: the
  commit that added it, or every change its `Credits` name ([RFD 121]).
  The release's notes branch then moves it, and until that merges it is still at
  the top level.
- A release is **in preparation** from the commit that starts it on its branch
  until its tag, and a tag is a **finalized** release.
- A finalized release is **built** when its build directory holds a build of the
  commit its tag names.

### Commands

[RFD 121] introduces the plugin with `init`, `check`, `highlight`, and a command
for each type of change, such as `fixed`.
This RFD adds:

| Command                                            | Does                                                                                             |
| -------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| `jp release cut [--version VERSION] [--at COMMIT]` | Starts the next minor or major release from `main`                                               |
| `jp release pick VERSION COMMIT...`                | Adds commits from `main` to a release in preparation, or starts the next patch release with them |
| `jp release finalize VERSION`                      | Tags a release, with its notes as the tag's message                                              |
| `jp release build VERSION`                         | Builds a release's binaries with the project's build command                                     |
| `jp release abandon VERSION`                       | Undoes a release nobody has published, so it can be cut again                                    |
| `jp release render [VERSION]`                      | Prints a release's notes or upgrade guide, or the unreleased fragments                           |
| `jp release due`                                   | Succeeds when the next release should be cut                                                     |
| `jp release status`                                | Says where the releases stand, and what to run next                                              |

`cut`, `pick`, `finalize`, `build`, and `abandon` read `main`, the release
branches, and the notes branches from `origin` and from the local repository,
and work in temporary worktrees, so the checkout they run from is left alone.
They make their branches and tags in the local repository and print the `git
push` that publishes them ([Publishing](#publishing)).
None is served as a tool: under [RFD 116]'s rule, commands that reach the
network or decide something for the project are the user's to run.

### Configuration

```toml
[plugins.command.release.options]
cadence = "2w"
forge = "github"
repository = "https://github.com/dcdpr/jp"
bump = "cargo set-version --package jp_cli {version}"
build = "bash .config/release/build.sh {version} {source} {out} {profile}"

[plugins.command.release.options.headings]
added = "New features"
fixed = "Bug fixes"
```

The example above shows JP's configuration.
Every option is optional:

- `cadence` defaults to two weeks, and takes a duration as `jp query --tmp`
  does.
- `forge` is `github`, `gitlab`, or `none`, and decides how releases credit and
  link change requests ([Credits and links](#credits-and-links)).
  Unset, it follows `repository`: `github` for github.com, `gitlab` for
  gitlab.com, and `none` for any other host.
  A self-hosted GitHub or GitLab sets it.
- `repository` is the web address links start from.
  Unset, it is the address of git's `origin` remote, so a clone whose `origin`
  is not the project's forge, such as a rehearsal's, sets it.
- `bump` has no default.
  Without it, a version lives only in its tag.
- `build` has no default, and has to name `{source}` and `{out}`; `builders`
  names commands that build elsewhere ([Building a
  release](#building-a-release)).
- `headings` renames a type's section in the release notes.
  A type it does not name keeps its default heading, its id capitalized
  (`Added`, `Performance`), and layered `headings` merge key by key.

`fragments_dir` is defined in [RFD 121].

### Versions

Git tags record what has been released: the current version is the highest
`v`-prefixed semantic-version tag, or `0.1.0` for the first release when no tag
exists.
Tags are annotated, and a release's date comes from its tag timestamp.

Cutting from `main` starts a minor release, or a major release (from `1.0`
onward) when an unreleased fragment has an upgrade section.
`--version` overrides the computed version, except that from `1.0` onward it
cannot name a minor release while an unreleased fragment has an upgrade section;
`cut` refuses that, as `pick` does.
The next version is computed from the highest tag or release in preparation, so
running `cut` while a release is in preparation cuts the subsequent version.
Patch releases are created only on release branches via `pick` ([Patch
releases](#patch-releases)).

When `bump` is set, the plugin runs it with `{version}` replaced by the new
version in the initial commit on the release branch, and for a cut also on the
notes branch, so `main` records the latest cut version once the notes merge.
This lets projects update version files wherever they keep them using their
existing tools.
JP's `bump` command runs `cargo set-version` (from `cargo-edit`) on `jp_cli`,
which updates `[workspace.package] version`, every crate that inherits it, and
`Cargo.lock` (checked in CI with `--locked`), while leaving the independently
versioned crates under `crates/contrib/` untouched.
Passing `--workspace` would overwrite those `crates/contrib/` versions as well.
JP's maintainers run these commands through `just release`, which installs a
pinned `cargo-edit`, builds `jp` and `jp-release` from the checkout, and runs `jp
release` with the given arguments.

### Cutting a release

`jp release cut`:

1. Rejects shallow clones (which lack the commit history needed for credits).
2. Reads `main` as the checked-out commit, which must contain every commit of
   `origin/main`: a checkout behind it would cut a release missing work already
   on `main`.
   A checkout ahead of it, such as a branch used for a rehearsal, is cut from,
   and `cut` reports that it is ahead.
3. Refuses to run while a release branch or `v` tag exists only in the local
   repository, since `cut` would count it as an existing release; a rehearsal
   leaves such branches and tags behind.
4. Refuses to run when `main` has no unreleased fragments, and computes the next
   version at that commit (the cut point).
5. Creates `release/<major>.<minor>` at the cut point, with one commit that
   deletes `changelog.d/` and runs `bump`.
6. Creates `release-notes/v<version>` from the same commit, with one commit that
   moves (`git mv`) every unreleased fragment and highlight into
   `changelog.d/<version>/` and runs `bump`.
7. Prints the `git push` that publishes both branches.

<!-- end list -->

```
changelog.d/
  README.md
  enable-explicit.removed.md    # merged after the cut: unreleased
  0.3.0/
    retry-idle-turns.fixed.md
    highlights/plugin-tools.md
```

Because both branches start from the same commit, the code on the release branch
and the fragments moved into `changelog.d/<version>/` match regardless of how
long the notes branch takes to merge.
Any fragment merged to `main` after the cut point remains at the root of
`changelog.d/`, and its code is not on the release branch, so both belong to the
next release.
If a correction to a moved fragment merges on `main` before the notes branch
merges, Git's rename tracking carries that edit into `changelog.d/<version>/`.
Because [RFD 121] requires every new change to add a new fragment file, any edit
to an existing fragment is a correction and belongs in the release that ships
that fragment.

Whether a fragment is unreleased depends on what release branches contain its
commit, not on its current directory path.
If a fragment is still at the root of `changelog.d/` only because another
release's notes branch has not merged yet, its commit is already on a release
branch, so a subsequent `cut` leaves it for that notes branch.
`cut` and `pick` never block on another release's notes.

Because the release branch deletes `changelog.d/`, release notes cannot be
edited on the release branch.
The version-bump commits that start releases are the only commits on a release
branch that do not originate on `main`.

### Cutting at an earlier commit

`jp release cut --at <commit>` cuts at a commit on `main`'s first-parent line
that no release ships yet, instead of at the commit checked out.
The release branch starts at that commit and the notes branch at the checkout,
which files only the fragments whose changes that commit contains.
So a project releases history it has no releases for, one period at a time, by
writing each period's fragments after the fact with `Credits` naming the changes
they describe ([RFD 121]); [RFD 123] does this for JP.
The commit that starts the release deletes `changelog.d/` only when the tree has
one, which such history does not, and git dates the commits and the tag as
`GIT_AUTHOR_DATE` and `GIT_COMMITTER_DATE` say, so a release cut after the fact
can carry the date it would have had.

### Publishing

`cut`, `pick`, `finalize`, and `abandon` publish nothing, and `build` writes
only to its build directory.
Each prints the `git push` that publishes what it made, such as:

```sh
git push origin release/0.3 release-notes/v0.3.0
```

The person releasing then runs it.
Until then, everything the command created is local: a mistake is undone by
deleting a branch or tag, and a whole release can be rehearsed on a separate
branch and deleted afterwards.

A release branch and a tag are pushed directly, and the notes branch is merged
into `main` like any other change.
`jp release` never talks to the forge, so it needs no token.

When both the local repository and `origin` have a branch, the commands use
whichever contains the other's commits; if the two have diverged, the commands
stop until they are reconciled.

### Preparing a release

Preparing release notes takes as long as needed without blocking `main`:

- **Notes** are edited on the notes branch until it merges, and on `main`, like
  any change, after: the wording of fragments and highlights in the release's
  directory, and its overview.
  Two entries for one change, such as a change and its follow-up, become one by
  editing one, deleting the other, and listing both change requests in the kept
  fragment's `Credits` fields.
- **The overview** opens the release notes.
  Without an `overview.md` in the release's directory, it is a summary generated
  at render: the version, how many entries of each type the release has, and its
  highlights' titles.
  Writing `overview.md` replaces it, and `jp release render <version>` prints
  the summary to start from.
- **More code** comes from `jp release pick 0.3.0 <commit>...`, with commits
  from the first-parent line of the commit checked out, which stands for `main`.
  It cherry-picks them onto `release/0.3` (`git cherry-pick -x`), leaving out
  what they change under `changelog.d/`.
  On the release's notes branch, with `main` merged in so that it holds the
  commits' fragments, or on a new notes branch from `main` when there is none,
  it puts each fragment and highlight the commits added into
  `changelog.d/0.3.0/`.
  A fragment no other release ships moves, so the next release does not list the
  change a second time.
  One that a later release already ships, because that release was cut after the
  commit merged, is copied, with its credits written into the copy's `Credits`
  fields ([RFD 121]): both releases ship the change, and both list it.
  When the notes branch has merged, a pick puts its notes on a new one.
  A notes branch that conflicts with `main` stops the pick before anything
  changes, until `main` is merged into it by hand, or, when it has merged, it is
  deleted.
  A conflict in the code stops `pick` in its worktree, which it names; resolve
  the conflict there and run `jp release pick --continue`.

Because code reaches a release branch only via `cut` or `pick`, every change
lands on `main` first.

### Finalizing a release

`jp release finalize 0.3.0` creates an annotated `v0.3.0` tag at the tip of
`release/0.3` in the local repository and prints the push that publishes it,
including the release branch when `origin` is missing some of its commits.
The tag message starts with the subject `Release 0.3.0` (shown by `git tag -n`),
followed by the release notes and, when the release has one, the upgrade guide
under an `Upgrade guide` heading, so `git show v0.3.0` shows the whole release
without the documentation site.
Breaking entries link to their upgrade sections by anchor only, since those
sections are in the same message; git keeps the message verbatim, including `#`
headings.

`finalize` reads the notes from `changelog.d/0.3.0/` in the checked-out commit,
which stands in for `main` as it does for `cut`, so the notes branch must merge
first.
It refuses to run while the notes branch (local or on `origin`) has a file in
the release's directory that the checked-out commit lacks, while a fragment for
a change the release ships is still at the root of `changelog.d/`, or while the
release has no notes at all.
A notes file deliberately deleted on `main` after the merge (such as one of two
entries for the same change) does not block it: deleting the merged notes branch
lets `finalize` proceed.

Pushing the tag publishes the release's code and notes, and building its
binaries comes next ([Building a release](#building-a-release)).
The person releasing decides when: on GitHub, CI runs on every push to a release
branch and shows on its tip; elsewhere, `just ci` runs the same checks locally.

The release's notes stay on `main`: correcting them, at any time, is a change to
its directory there, and every place the notes appear renders them again, except
the tag, which keeps the notes the release was tagged with.

### Building a release

`jp release build 0.3.0` builds a release's binaries on this machine:

1. It checks out the commit `v0.3.0` names in a temporary worktree, or, for a
   release in preparation, its branch's tip.
2. It runs the `build` command from the root of the checkout, so an old release
   builds with today's build definition, with `{version}`, `{source}` (the
   worktree), `{out}` (an empty directory), and `{profile}`: `release`, or
   `quick` with `--quick`.
   What the command prints is shown as it runs, and `--quiet` hides it.
3. It checks what the command left in `{out}`: the archives, `checksums.txt`
   with a SHA-256 for each, and `manifest.json`, which names the version,
   commit, and profile built and the platform each archive is for, and nothing
   else.
   A build that fails, or whose output breaks any of this, is not kept, and the
   check names every problem at once.
4. It moves the result to `release/0.3.0/` in the workspace's user-local storage
   (`jp path user-workspace`), replacing an earlier build of that version.

<!-- end list -->

```json
{
  "format": 1,
  "version": "0.3.0",
  "commit": "8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b",
  "profile": "release",
  "artifacts": [
    { "file": "jp-aarch64-apple-darwin.tar.xz", "target": "aarch64-apple-darwin" },
    { "file": "jp-x86_64-pc-windows-msvc.zip", "target": "x86_64-pc-windows-msvc" }
  ]
}
```

A build made with the `release` profile from the commit its version's tag names
is **publishable**, so a build of a release in preparation becomes publishable
when `finalize` tags the commit it was made from.
A `--quick` build, and a build of a commit the tag doesn't name, such as one
left from before an abandon, are for testing, and publishers refuse them ([RFD
123]).
`jp release status` names each finalized release without a publishable build.

A **remote builder** builds elsewhere, such as on dedicated hardware a CI job
drives.
`builders.<name>` is a command that starts a build there and fetches its result
into `{out}`, and `jp release build 0.3.0 --remote <name>` runs it, after
checking that `origin` has the tag the builder builds, then checks the result
the same way.
A builder that signs or attests what it builds does so over its own
`checksums.txt`, which reaches the build directory unchanged.

### Patch releases

Patch releases branch from their release series branch rather than `main`.
After `v0.3.0` is tagged, `jp release pick 0.3.1 <commit>...` starts `0.3.1` by
adding a version-bump commit (`bump`) on `release/0.3`, cherry-picking the
requested commits, and moving or copying their fragments into
`changelog.d/0.3.1/` on a new notes branch, `release-notes/v0.3.1`.
`pick` rejects fragments that contain an upgrade section when targeting a patch
release.
`jp release finalize 0.3.1` then tags `v0.3.1`.
`pick` only accepts a version that is already in preparation or is the immediate
next patch version for a tagged release series.

### Releases in parallel

Each release branch holds one release in preparation at a time: a patch release
starts after the previous release on its line is tagged.
Releases on different branches do not touch each other.
`release/0.3` can prepare 0.3.1 while `main` cuts 0.4.0, since each writes its
own branches and its own version's directory.

### Abandoning a release

`jp release abandon 0.3.0` undoes a release that `origin` has no tag for, so it
can be cut again:

- It refuses while a later release exists, so releases are abandoned newest
  first, and refuses a release `origin` has tagged, which is published.
- It deletes the release's tag and notes branch here, and its release branch,
  or, for a patch release, the commits from the one that started it.
- On `release-notes/abandon-v0.3.0`, branched from the release's notes branch if
  that has not merged and from the checkout if it has, it moves the release's
  fragments and highlights back to the top level with their wording as edited.
  A fragment `pick` copied from another release is deleted instead, and
  `overview.md` stays in the release's directory, where the next cut of that
  version finds it.
- It prints the pushes that remove from `origin` what it removed here.
  The new notes branch is merged into `main` like any other.

### Release notes and the upgrade guide

Each release produces two documents:

- **Release notes**: the overview, the highlights, and all fragments grouped by
  type under their section headings.
  Breaking entries are marked and link to their corresponding upgrade guide
  section.
- **Upgrade guide**: one section per fragment that includes an upgrade section
  ([RFD 121]), using the upgrade title as the heading followed by the entry's
  credits and upgrade instructions.
  Releases without upgrade sections omit the upgrade guide.

Example release notes from `jp release render 0.3.0` (entries are paraphrased
from recent pull requests, with an invented breaking change and final entry):

```markdown
JP 0.3.0 lets the assistant call one tool on many inputs at once, and lets the
plugins you install give it new tools. It also changes how JP finds plugins, so
read the upgrade guide before you upgrade.

## Tool calls fan out

When the assistant needs the same tool on several inputs, such as reading ten
files, it asks once and JP runs the calls side by side. Each result still shows
up under its own call, and one failing call does not stop the others.

By @JeanMertz in [#1182](https://github.com/dcdpr/jp/pull/1182) and
[#1183](https://github.com/dcdpr/jp/pull/1183).

## Changelog

### New features

- The assistant can call one tool on many inputs in a single request.
  ([#1182](https://github.com/dcdpr/jp/pull/1182))
- `jp conversation show` shows the conversation's model.
  ([#1243](https://github.com/dcdpr/jp/pull/1243))

### Changed

- JP finds a command plugin by the manifest its binary embeds, not by its file
  name. **Breaking**: see
  [the upgrade guide](upgrade#command-plugins-are-found-by-their-manifest).
  ([#1216](https://github.com/dcdpr/jp/pull/1216))

### Bug fixes

- A turn that stalls while idle is retried instead of failing.
  ([#1207](https://github.com/dcdpr/jp/pull/1207))
- The help for `jp query --tmp` names the right default.
  ([`4e1f0a2`](https://github.com/dcdpr/jp/commit/4e1f0a2))
```

The corresponding upgrade guide from `jp release render 0.3.0 --page upgrade`:

````markdown
## Command plugins are found by their manifest

[#1216](https://github.com/dcdpr/jp/pull/1216)

Before 0.3.0, `jp <name>` ran a binary called `jp-<name>` from your `PATH`.
JP now reads the manifest each plugin binary embeds, which names the commands
it serves, and a plugin built before 0.3.0 has none, so JP no longer finds it.

Rebuild the plugin against 0.3.0, or reinstall it:

```sh
jp plugin install <name>
```
````

Neither output includes a top-level document title or release date; the consumer
adds both when publishing the document.

### The release model

The `changelog` crate builds and serializes a release model from a
`changelog.d/<version>/` directory or from everything no finalized release ships.
That unreleased model holds the fragments at the root of `changelog.d/` and the
directory of every release without a tag, so a release's entries count as
unreleased until it is tagged, however early its notes branch merged.
Markdown is one rendering of that model; consumers that need custom formatting
read the JSON output:

```json
{
  "format": 1,
  "version": "0.3.0",
  "date": "2026-10-28",
  "overview": "JP 0.3.0 lets the assistant call one tool on many inputs ...",
  "highlights": [
    {
      "slug": "fan-out",
      "title": "Tool calls fan out",
      "authors": ["@JeanMertz"],
      "body": "When the assistant needs the same tool on several inputs ...",
      "credits": [
        { "kind": "change_request", "label": "#1182", "url": "https://github.com/dcdpr/jp/pull/1182" }
      ]
    }
  ],
  "entries": [
    {
      "slug": "plugin-manifest-routing",
      "type": "changed",
      "body": "JP finds a command plugin by the manifest its binary embeds, ...",
      "credits": [
        { "kind": "change_request", "label": "#1216", "url": "https://github.com/dcdpr/jp/pull/1216" }
      ],
      "upgrade": {
        "title": "Command plugins are found by their manifest",
        "body": "Before 0.3.0, `jp <name>` ran a binary called ..."
      }
    }
  ]
}
```

- `overview`, `title`, and `body` fields are Markdown strings.
  When `overview.md` is absent, `overview` contains the generated summary.
- `type` is the raw fragment type ID from [RFD 121], never a display heading;
  consumers map type IDs to headings.
- Each credit has a `kind` (`change_request` or `commit`) and a forge-formatted
  `label` (`#1182` on GitHub, `!1182` on GitLab, or a short commit hash), so
  consumers can render `[label](url)` without forge-specific logic.
  When `forge = "none"`, credits are commits with no `url`.
- `date` is `null` for a release in preparation, and both `version` and `date`
  are `null` for the unreleased model.
- Within each type, entries are sorted by their first credit and then by `slug`
  for deterministic output.
- `format` versions the JSON schema so consumers can reject unsupported
  versions.

Both `changelog render` (the standalone crate binary) and `jp release render`
accept `--format json` or `--format markdown`, plus `--page notes` or `--page
upgrade` for Markdown output.
The `changelog` binary does not read JP configuration; it receives the fragments
directory, forge, project web address (`repository`), and section headings via
CLI flags, which `jp release render` populates from the plugin configuration.
In Markdown mode, breaking entries link to their upgrade guide section using a
base path passed by the caller.

Every consumer renders from `main`, where the release notes live:

- `jp release render` prints them, to read while a release is prepared or to
  hand to another tool.
- `jp vite` turns them into pages on the site ([On the site](#on-the-site)).
- A publisher, such as [RFD 123]'s `jp release github`, puts them where it
  publishes the release, and renders them again when the release's directory
  changes on `main`.

### Credits and links

Credits come from `main`'s history.
For each fragment and highlight, the crate reads the commits that wrote or
changed it while it sat at the top level of `changelog.d/`, following renames,
up to the commit that moved it into a release's directory.
Corrections after that credit nobody.
A commit's change request is read from its message or, when the commit reached
`main` through a merge, from the merge commit's message.
A fragment that lists `Credits` takes those instead, which is how a copy made
for a patch release keeps the original's credits, commits included.
Links start from `repository`, or from the web address of git's `origin` when it
is unset.

| `forge`  | Change request pattern                                                       | Label  | Target URL                                      |
| -------- | ---------------------------------------------------------------------------- | ------ | ----------------------------------------------- |
| `github` | `(#123)` at the end of a squash commit subject, or `Merge pull request #123` | `#123` | `https://<host>/<owner>/<repo>/pull/123`        |
| `gitlab` | `See merge request <project>!123` (GitLab's default merge commit trailer)    | `!123` | `https://<host>/<project>/-/merge_requests/123` |
| `none`   | None                                                                         |        |                                                 |

Commits without a matching change request reference are credited by short commit
hash and linked to the forge's commit page (`/commit/<sha>` on GitHub,
`/-/commit/<sha>` on GitLab).
Because JP squash-merges pull requests using GitHub's default commit subject,
its entries credit pull requests automatically.
GitLab's default squash commit message contains only the merge request title, so
GitLab projects that squash-merge receive commit credits unless their squash
commit template ends with `See merge request %{reference}`, as GitLab's default
merge commit message does.

Because credit resolution requires Git history, `render` rejects shallow clones
rather than emitting incomplete credits.

### Release workflow

`jp release due` exits zero when `cadence` has elapsed since the latest minor or
major release was cut, that release has been finalized, and `main` has at least
one unreleased fragment (without one, `cut` would have nothing to release).

1. **Cut.** A maintainer runs `jp release cut`, when `due` says so or on any
   other day, and pushes the two branches it made.
2. **Prepare.** They and others edit the release's notes on its notes branch,
   and pick later work onto its release branch, for as long as it takes, then
   merge the notes branch into `main`.
3. **Finalize.** They run `jp release finalize` from `main` once the notes
   branch has merged and the release branch's checks pass, and push the tag.
4. **Build.** They run `jp release build`, and publish what it built wherever
   the project publishes releases ([RFD 123]).

A patch release is a `pick` into the next patch version, then `finalize` and
`build`.

`jp release status` reports the next step, so maintainers do not have to
remember the order.
It reads the same state as the other commands without changing anything, and
reports, in this order:

- whether the checkout is missing commits from `origin/main`, which `cut` and
  `finalize` refuse;
- a pick a conflict stopped, and the `--continue` that finishes it;
- the release branches, notes branches, and `v` tags only the local repository
  has, with the push that publishes them, or the commands that delete what a
  rehearsal left; a notes branch the checkout has merged is left out;
- each release in preparation, with where its notes are: on its notes branch,
  with the merge still to come; or on `main`, with the `finalize` that tags it;
- each finalized release without a publishable build, with the `build` that
  makes one;
- while no minor or major release is in preparation, `due`'s answer, with the
  `cut` that starts the next one when it is due.

### JP's CI

Format validation, the checks on pushes, `due`, and the site build invoke `just`
recipes, so the same steps can run locally without GitHub.
The changelog gate, the release reminder, and the site build on a tag depend on
GitHub labels, issues, and workflow runs, which they access with `gh`.
No workflow pushes commits to any branch:

| When                            | Runs                                                                                            | Writes                                                             |
| ------------------------------- | ----------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| A pull request                  | The changelog gate and format validation ([RFD 121])                                        | Nothing                                                            |
| A push to `main` or `release/*` | The checks; on `main`, `just changelog-ci` among them, since release branches have no fragments | Nothing                                                            |
| Daily                           | `just release-due`                                                                              | An issue labelled `release` when a release is due and none is open |
| A push to `main`                | The site build ([On the site](#on-the-site))                                                    | The site                                                           |
| A push of a `v*` tag            | The site build on `main`, which the release workflow starts ([On the site](#on-the-site))       | The site                                                           |
| A push of a `vX.Y.0` tag        | Closing the reminder, since a minor or major release answers it                                 | Closes the reminder issue                                          |

Because maintainers push branches and tags themselves, downstream workflows
trigger normally.
In GitHub Actions, pushes and pull requests created with `GITHUB_TOKEN` do not
trigger other workflows and require manual approval to run checks ([Triggering a
workflow from a workflow]); because CI never pushes branches or opens pull
requests, the pipeline needs no bot account or token workarounds.
The only run a workflow starts is the site build for a release's tag, through
`workflow_dispatch`, which a workflow's own token is allowed to trigger.

### On the site

Releases are a third workflow for `jp vite` ([RFD 116]), next to tickets and
RFDs.
`jp vite sync` writes `fragments_dir`, `forge`, `repository`, and `headings`
into its settings snapshot with the other plugins' settings.
The releases loader runs `changelog render --format json` for each version's
directory and for the unreleased model, with arguments from the snapshot, and
builds the pages from the model: release notes and an upgrade guide for each
finalized release, and an "Unreleased" page.
A release gets its pages when it is tagged: until then, its entries are on the
Unreleased page, grouped by type with the fragments no release ships yet.
Under `jp vite serve`, a release in preparation also gets its pages, marked as a
draft, so its notes can be read as they will appear; the built site, which `just
preview-docs` serves, shows them as they deploy.
`jp release render 0.3.0` prints them without the site.
The pages are never committed.
Where they go is `jp vite`'s choice: JP serves them under `/releases/`, with an
index of releases and their dates, and replaces the navigation's "Change Log"
entry with "Releases".

Under `jp vite serve`, and under `jp vite build`, which builds the site as
`vitepress build` does, the loader renders with the `jp-vite` binary itself, run
as `jp-vite changelog render`, which `jp vite` names to it in `JP_VITE_BIN`.
A workspace then needs nothing but `jp` and its plugins to build its site, and
the release pages are rendered by the release of `jp vite` that wrote the site's
files.
`jp vite build` brings the settings snapshot up to date first, as `serve` does,
and refuses while a file `jp vite sync` writes is out of date.
A `vitepress build` started by hand runs the `changelog` binary on `PATH`.

JP's docs build runs `vitepress build` directly, so it needs no `jp`; it
compiles and installs the `changelog` crate binary in place of `jilu`, without
compiling the rest of the workspace.
The docs workflow already fetches full Git history, which credit resolution
requires.

Release pages are always built from `main`.
Because release tags do not contain `changelog.d/` and `jp vite` only enables a
workflow when its directory exists ([RFD 116]), building the site from a release
tag would omit all release pages and must not overwrite the deployment from
`main`.
JP's site workflow therefore builds only from `main`; pushing a release's tag
starts it there through the release workflow, which is when the release's pages
appear.
If JP later adds versioned documentation per release, each tag can build into a
versioned subpath and link to the `/releases/` pages deployed from `main`.

### Other artifacts

Command plugins are published continuously via the rolling `plugins` release, so
plugin changes reach users before their changelog fragments are collected into a
numbered JP release.
Although plugin crates inherit `[workspace.package] version`, the `plugins.yml`
workflow does not rebuild plugins on a version-bump commit, so rolling plugin
binaries report the prior workspace version until their next code change.
A separate RFD on plugin distribution will decide whether numbered releases also
publish versioned plugin binaries.

The macOS app ([RFD 099]) is not distributed as part of JP releases and
maintains its own version.

### Retiring `jilu`

Generated `/releases/` pages replace the committed `docs/change-log.md` file.
The `build-changelog` recipe in the `justfile`, the `jilu` dependency, and the
docs-build step that ran `jilu` are removed.

## Drawbacks

- **The project owns the assembly logic.** `jilu` is replaced by code JP
  maintains.
  The logic is small (parse, credit, model, render), but it is now ours to test
  and fix.
- **A cut or pick lands in two steps.** The release branch is pushed at once;
  `main` follows when the notes branch merges, and until then the release's
  fragments are still at the top level.
- **Publishing is a step to forget.** The commands push nothing, so a cut nobody
  pushed blocks the next cut until it is pushed or deleted.
- **A local build is as trustworthy as the machine it ran on.** It carries
  checksums, which show a file is the one published but not how it was built,
  until a remote builder that attests its builds replaces it.
- **`pick` merges `main` into the notes branch.** A notes branch edited where
  `main` changed too stops the pick until someone merges `main` into it by hand.
- **Branches and directories accumulate**: a release branch per minor release
  and a directory per release, 26 of each a year at JP's cadence.
- **Tags off `main`.** Every release is tagged on its branch, past the commit
  that deletes `changelog.d/`, so `git describe` on `main` finds no release.
  A release's starting point on `main` is the parent of its branch's first
  commit.
- **Workflows that run `jp release` compile `jp` and `jp-release`** until
  releases publish both; the daily `due` is the main one.
- **Credits depend on merge messages.** A forge or project that writes no change
  request into its merge or squash commits gets commit credits instead.

## Alternatives

- **A `just`-invoked binary instead of a command plugin.** Free of the plugin
  system, but it splits release tooling from the plugin family other workspaces
  install.
- **Release pull requests into the release branch, with the notes synced back to
  `main` afterwards.** The notes would live on both branches, and correcting an
  old release would have two possible places, one of them wrong.
- **Committing the cut to `main` directly.** One step instead of two, but on
  GitHub it needs a bypass of `main`'s pull-request rule for whoever cuts
  releases, which lets them push anything else too, and it puts unreviewed
  commits on `main`.
  Starting the release branch and the notes branch from the same commit makes
  the cut just as atomic.
- **Orphan release branches**, with no history shared with `main`, so a release
  branch could not be merged back.
  Nothing would come back anyway: every commit on a release branch comes from
  `main` except the ones that start releases, and a merge would delete every
  release note, which nobody merges unnoticed.
  The cost would land on everyday debugging: no `git bisect` between two release
  tags, no `git log v0.3.0..v0.4.0` of what changed between releases, and no
  link in git from a tag to the commit of `main` it was cut from.
- **No release branches: tagging `main` where a release is cut.** One branch
  fewer, but no picks, and no patch release without everything else merged
  since.
- **Preparing on a short-lived branch merged into `main`.** Whatever merges to
  `main` meanwhile ships unless someone holds it back.
- **Freezing `main` during a release**, as Vector's release checklist does.
  Everyone else waits while the notes are prepared.
- **Commands that push, and open the pull request through the forge.** One step
  fewer, but `jp release` would need a token and code for each forge, and a cut
  whose second push failed would be left half done, with nothing to finish it.
  A rehearsal would need a stand-in remote.
  The forge answers a push with the address that opens the pull request anyway.
- **Building on the forge when a tag is pushed**, as release tools that generate
  CI workflows do.
  Builds would come from the forge's runners alone, a release could not be built
  before its tag reached the forge, and a project leaving the forge would
  rebuild its release process.
  A remote builder runs on such hardware through the same command as a local
  build.
- **A pull request per pick**, from a branch of its own off `main`.
  No merge into an open notes branch, but one more pull request per pick, and a
  release's notes spread over several.
- **Workflows that prepare releases or open pull requests.** A workflow's pushes
  and pull requests wait for approval unless it holds a GitHub App token, and
  either way the flow needs GitHub.
- **A check on `main` that fails when a release is overdue.** Every pull request
  would show it red, and no change in it can fix it, which teaches people to
  ignore red.
- **An overview written at the cut.** Picks change what it summarizes, and
  regenerating it until someone edits it is state nobody can see.
- **Committing rendered pages.** The site would need no `changelog` binary, but
  each page would be frozen at release time: a typo fix or a format change would
  mean editing generated Markdown.
- **Deleting fragments once released.** A smaller tree, with the same frozen
  pages.
- **Version handling built in for Rust workspaces.** It would tie `cut` to one
  kind of project, where a command reaches the same result with tools projects
  already have.
- **Reading the version from a manifest.** Tags record what was released, and a
  file that is only written cannot drift from them.

## Non-Goals

- **Publishing.** `jp release` ends with a pushed tag and a local build; putting
  a release's binaries and notes on a forge, a website, or object storage is a
  publisher's job, and [RFD 123] adds GitHub's.
  Until releases publish `jp` and `jp-release`, JP's release workflows compile
  them from the checkout.
- **What a project builds.** Its `build` command decides targets, toolchains,
  and archives; [RFD 123] defines JP's.
- **Signing.** A remote builder can sign or attest what it builds; a local build
  carries checksums alone.
- **Pre-releases.** A release candidate must not consume the fragments its final
  release needs, which needs its own design.
- **Independent package versions.** One release, one version.
- **CI for other workspaces.** JP's workflows are JP's, and `jp release` needs
  none.
- **Publishing to crates.io.** JP ships as binaries.

## Risks and Open Questions

- **Configuration without `jp`.** JP's release workflows run `jp` because only
  `jp` resolves configuration; the site does not.
  A follow-up RFD can give plugins a standalone mode that resolves configuration
  with the same code `jp` uses; the `changelog` crate reads none, so only the
  plugins would change.
  Release binaries make `jp` a download rather than a compile in the meantime.
- **A breaking change that has to ship in a release that cannot break.** `pick`
  refuses a fragment with an upgrade section into a patch release, and from
  `1.0` on into a minor one, and `cut --version` refuses such a minor release.
  Shipping it means `jp release abandon` on the release in preparation and
  cutting a major one.
- **Abandoning a pushed patch release** removes commits from its release branch,
  a force push that the protection on `release/*` refuses.
  A pushed patch release is finished rather than abandoned.
- **Lockfile conflicts.** The notes branch changes `Cargo.lock` through `bump`,
  and stays open while the notes are prepared, so a change to `Cargo.lock`
  merged meanwhile may conflict with it; running `bump` again after merging
  `main` into it resolves it.
- **Highlights extended across a cut.** Highlights are edited by every pull
  request that builds their feature ([RFD 121]), so a pull request merged after
  the cut that extends a highlight the cut moves has its extension carried into
  the released copy, describing work the release does not ship.
  Fragments avoid this by being added rather than extended; highlights cannot,
  since a highlight is one story per feature.
  The release's highlights are read while its notes are prepared, and a
  correction is an edit to the released copy on `main`.

## Implementation Plan

### Phase 1: Model, credits, and rendering

Implement the release model, forge credit resolution (`github`, `gitlab`, and
`none`), and Markdown/JSON rendering in `crates/internal/changelog` (`changelog
render`), and expose `jp release render` with the `forge` and `headings`
options.
Depends on [RFD 121] Phases 2 and 3.

### Phase 2: `cut`, `pick`, and `finalize`

Implement version computation from tags and release branches, the `bump` hook,
release branch and notes branch creation in the local repository from the
checked-out commit (printing the `git push` that publishes them), `pick`
(including fragment moves and copies onto the notes branch), patch releases,
and `finalize` (with the release notes as the tag message).
Configure `bump` for JP, and enable CI checks on pushes to `release/*`.
Depends on Phase 1.

### Phase 3: `due` and the reminder workflow

Implement `jp release due` and `jp release status`, add the `just release-due`
recipe, and add the daily GitHub Actions workflow that opens a
`release`-labelled reminder issue (closed automatically when the tag of the next
minor or major release is pushed).
Depends on Phase 2.

### Repository settings

No repository settings block Phase 2: `main`'s branch protection already accepts
the notes branches' pull requests, and no rules currently restrict `release/*`,
so release branches can be pushed.
Once Phase 2 merges, a repository administrator should configure branch and tag
protection to prevent history rewrites:

- On `release/*`: block force pushes and branch deletion.
- On `v*` tags: block tag updates and deletion.

T-1f2jmb7 tracks applying them.

### Phase 4: Documentation site and retiring `jilu`

Add the releases workflow to `jp vite`: snapshot `fragments_dir`, `forge`, and
`headings` in `jp vite sync`, add the releases loader, generate the release
notes, upgrade guide, index, and "Unreleased" pages, and replace "Change Log"
with "Releases" in the site navigation.
Update the docs workflow to build `changelog` instead of `jilu`, build the site
only from `main` (and again when a release's tag is pushed), and remove `jilu`
and `docs/change-log.md`.
Depends on Phase 1 and the site phase of [RFD 116].

### Phase 5: Cutting the past, and abandoning

`cut --at`, with fragments filed by the changes their `Credits` name, release
branches started from trees without `changelog.d/`, and `abandon`.
`cut` tells the person releasing to merge the notes branch into `main`.
Depends on Phase 2.

### Phase 6: Building

`build` with the `build` option and `--quick`, the build directory and its
checks, and `status` naming finalized releases without a publishable build.
`--remote` and `builders` land with a project's first remote builder.
Depends on Phase 2.

### Phase 7: Links and drafts

The `repository` option, wherever notes are rendered and in `jp vite`'s
snapshot, and draft pages for releases in preparation under `jp vite serve`.
Depends on Phase 4.

### Phase 8: Sites built with `jp vite`

`jp vite build`, and the `jp-vite` binary rendering the release pages under
`serve` and `build`, as `jp-vite changelog`, so only a `vitepress build` started
by hand, as JP's docs build is, needs the `changelog` binary.
Depends on Phase 4.

## References

- [RFD 116: Shareable RFD, Ticket, and Site Tooling][RFD 116], the plugin family
  and the `jp vite` workflows this RFD adds to.
- [RFD 121: Changelog Fragments and Validation][RFD 121], the inputs this
  pipeline consumes and the plugin it extends.
- [RFD 123: Distributing the jp Binary][RFD 123], what JP builds and where it
  publishes it.
- [RFD 001: The JP RFD Process][RFD 001], the lifecycle this RFD follows.
- [Bevy release news example] and [Bevy migration guide example], the split
  between release notes and upgrade guide, and [bevy-website
  `generate-release`], prior art for assembling them from fragments.
- [Vector releases index], prior art for the per-release page and index, and
  [Vector's release checklist], the freeze this RFD avoids.
- [Triggering a workflow from a workflow], GitHub's rules for what the workflow
  token can and cannot start.
- [GitLab commit message templates], what GitLab writes into merge and squash
  commits.
- [cargo-edit], whose `cargo set-version` is JP's `bump`.

[Bevy migration guide example]: https://bevy.org/learn/migration-guides/0-18-to-0-19/
[Bevy release news example]: https://bevy.org/news/bevy-0-19/
[GitLab commit message templates]: https://docs.gitlab.com/user/project/merge_requests/commit_templates/
[RFD 001]: 001-jp-rfd-process.md
[RFD 099]: 099-native-macos-app-for-browsing-conversations.md
[RFD 116]: 116-shareable-rfd-ticket-and-site-tooling.md
[RFD 121]: 121-changelog-fragments-and-validation.md
[RFD 123]: 123-distributing-the-jp-binary.md
[Triggering a workflow from a workflow]: https://docs.github.com/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow#triggering-a-workflow-from-a-workflow
[Vector releases index]: https://vector.dev/releases/
[Vector's release checklist]: https://github.com/vectordotdev/vector/blob/master/.github/ISSUE_TEMPLATE/minor-release.md
[bevy-website `generate-release`]: https://github.com/bevyengine/bevy-website/tree/main/generate-release
[cargo-edit]: https://github.com/killercup/cargo-edit
