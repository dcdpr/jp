# RFD 122: Release Pipeline

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-01
- **Extends**: [RFD 116]
- **Requires**: [RFD 121]
- **Required by**: [RFD 123]
- **Summary**: The `jp release` plugin cuts releases onto branches, files their
  notes on `main` through a pull request, and tags them, all in the local
  repository for the person releasing to push.

## Summary

This RFD extends the `jp release` plugin ([RFD 121]) with commands that cut,
patch, and tag releases while keeping all release notes and upgrade guides on
`main`.
`jp release cut` branches `release/<series>` for code and
`release-notes/v<version>` for moving unreleased fragments into the release's
directory, which reaches `main` through a pull request.
`jp release pick` cherry-picks fixes onto the release branch, and `jp release
finalize` tags the release, with its release notes as the tag message.
Each command works in the local repository and prints the `git push` that
publishes its result, so a release can be rehearsed without publishing anything.
The pipeline replaces `jilu`; [RFD 123] covers binary builds and distribution.

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

## Design

### Repository layout

| Location               | Contents                                                                                                                                |
| ---------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| `main`                 | Active development and all release notes: unreleased fragments at the root of `changelog.d/`, released ones in `changelog.d/<version>/` |
| `release/0.3`          | The code shipped in `0.3.x` releases: `main` at the cut point, without `changelog.d/`, plus cherry-picked commits                       |
| `release-notes/v0.3.0` | A release's notes on their way to `main`: its fragments moved into `changelog.d/<version>/`, merged through a pull request             |
| `v0.3.0`, `v0.3.1`     | Annotated tags on `release/0.3` marking finalized releases                                                                              |

No separate state file is used:

- A release **ships** a change when its branch contains the commit that
  introduced it, either from `main` at the cut point or via `jp release pick`.
- A fragment in `changelog.d/<version>/` belongs to that release.
  A fragment at the root of `changelog.d/` is **unreleased** until a release
  branch contains its commit.
  The release's notes branch then moves the fragment into
  `changelog.d/<version>/`; until that branch merges, the fragment stays at the
  root.
- A release is **in preparation** from the initial commit on its release branch
  until it is tagged.
  Tagging produces a **finalized** release.

### Commands

[RFD 121] introduces the plugin with `init`, `check`, `highlight`, and a command
for each type of change, such as `fixed`.
This RFD adds:

| Command                              | Description                                                                                      |
| ------------------------------------ | ------------------------------------------------------------------------------------------------ |
| `jp release cut [--version VERSION]` | Starts the next minor or major release from `main`                                               |
| `jp release pick VERSION COMMIT...`  | Cherry-picks commits from `main` onto a release in preparation, or starts the next patch release |
| `jp release finalize VERSION`        | Tags a finalized release, with its release notes as the tag message                              |
| `jp release render [VERSION]`        | Renders a release's notes or upgrade guide, or the unreleased fragments                          |
| `jp release due`                     | Exits zero when the next release is due                                                          |

`cut`, `pick`, and `finalize` read `main`, the release branches, and the notes
branches from both `origin` and the local repository, and operate in temporary
worktrees, leaving the caller's working tree untouched.
They create branches and tags in the local repository and print the `git push`
that publishes them ([Publishing](#publishing)).
Following [RFD 116], none of these commands are exposed as LLM tools because
they access the network or make project-level release decisions.

### Configuration

```toml
[plugins.command.release.options]
cadence = "2w"
forge = "github"
bump = "cargo set-version --package jp_cli {version}"

[plugins.command.release.options.headings]
added = "New features"
fixed = "Bug fixes"
```

The example above shows JP's configuration.
Every option is optional:

- `cadence` defaults to two weeks (`2w`) and accepts the same duration format as
  `jp query --tmp`.
- `forge` is `github`, `gitlab`, or `none`.
  It controls how release notes credit and link change requests ([Credits and
  links](#credits-and-links)).
  When unset, it is inferred from the `origin` remote URL: `github` for
  `github.com`, `gitlab` for `gitlab.com`, and `none` for any other host.
  Self-hosted GitHub or GitLab instances set `forge` explicitly and use the
  remote's hostname for links.
- `bump` has no default.
  When unset, the version is recorded only in the Git tag.
- `headings` maps fragment type IDs to section headings in the release notes.
  Unlisted types default to their capitalized ID (`Added`, `Performance`).
  Across config layers, `headings` merges key by key.

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

Because JP tagged `v0.1.0` without publishing a release before this pipeline
existed, its first published release is `v0.2.0`, covering changes merged after
[RFD 121]'s check landed.
A release meant to introduce JP to new users states that in its overview and
highlights; the pipeline treats every release identically.

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

### Publishing

`cut`, `pick`, and `finalize` publish nothing themselves.
Each prints the `git push` that publishes what it created, for example:

```sh
git push origin release/0.3 release-notes/v0.3.0
```

The person releasing then runs it.
Until then, everything the command created is local: a mistake is undone by
deleting a branch or tag, and a whole release can be rehearsed on a separate
branch and deleted afterwards.

Release branches and tags are pushed directly.
The notes branch reaches `main` through a pull request like any other change, so
CI checks, review, and merging work the same way as on any contributor change
request.
GitHub and GitLab respond to the push with the URL for opening that pull request.
`jp release` never calls the forge's API, so it needs no token, and projects
without a forge merge the notes branch through their usual workflow.

When both the local repository and `origin` have a branch, the commands use
whichever contains the other's commits; if the two have diverged, the commands
stop until they are reconciled.

### Preparing a release

Preparing release notes takes as long as needed without blocking `main`:

- **Editing notes**: Fragment files, highlights, and the overview in
  `changelog.d/<version>/` are edited on the notes branch while its pull request
  is open, and on `main` through normal change requests once it has merged.
  To combine two fragments for a single logical change (such as a feature and a
  follow-up fix), edit one fragment, delete the other, and list both change
  requests in the surviving fragment's `Credits` fields.
- **Overview**: The overview introduces the release notes.
  When `changelog.d/<version>/overview.md` is absent, rendering generates a
  default summary stating the version, the count of entries per type, and the
  highlight titles.
  Adding `overview.md` replaces the generated summary; `jp release render
  <version>` prints the generated text as a starting point.
- **Cherry-picking commits**: `jp release pick 0.3.0 <commit>...` takes commits
  from the first-parent history of the checked-out commit, which stands in for
  `main`.
  It cherry-picks them onto `release/0.3` (`git cherry-pick -x`) and strips any
  changes under `changelog.d/`.
  It then places the fragments and highlights introduced by those commits into
  `changelog.d/0.3.0/` on the release's notes branch, after merging `main` into
  it so the branch contains them, or on a new notes branch from `main` if none
  exists.
  If no later release ships the fragment, `pick` moves it so the next release
  does not list it again.
  If a later release was already cut after the commit merged to `main`, `pick`
  copies the fragment and writes its resolved credits into the copy's `Credits`
  fields ([RFD 121]), so both releases include the change and list the entry.
  If the notes branch's pull request has already merged, the picked notes go to
  a new pull request from that branch.
  If the notes branch conflicts with `main`, `pick` stops before changing
  anything, until `main` is merged into the notes branch by hand or, if its pull
  request has already merged, the branch is deleted.
  If cherry-picking hits a merge conflict in the code, `pick` stops and prints
  the temporary worktree path; after resolving the conflict there, run `jp
  release pick --continue`.

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

Pushing the tag is the release.
Maintainers run `finalize` once checks pass on the release branch (via GitHub CI
on `release/*` pushes, or `just ci` locally).

Because a maintainer pushes the tag directly, the push triggers JP's binary
distribution workflow ([RFD 123]).
Release notes remain editable on `main` after tagging: updating files in
`changelog.d/<version>/` re-renders the notes wherever they are published,
except in the tag, which keeps the notes the release was tagged with.

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

Each `release/<major>.<minor>` branch has at most one release in preparation at
a time; a patch release cannot start until the preceding release on that branch
is tagged.
Different release branches are independent: `release/0.3` can prepare `0.3.1`
while `main` cuts `0.4.0`, since each uses its own branches and its own
`changelog.d/<version>/` directory.

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
`changelog.d/<version>/` directory or from the unreleased fragments at the root
of `changelog.d/`.
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
directory, forge, and section headings via CLI flags, which `jp release render`
populates from the plugin configuration.
In Markdown mode, breaking entries link to their upgrade guide section using a
base path passed by the caller.

Every consumer renders from `main`, where the release notes live:

- `jp release render` prints notes or upgrade guides for local review or
  external tooling.
- `jp vite` renders them as documentation site pages ([On the
  site](#on-the-site)).
- The binary distribution workflow ([RFD 123]) publishes the release notes to
  the GitHub Release with links to the upgrade guide on the documentation site,
  and updates the GitHub Release whenever `changelog.d/<version>/` changes on
  `main`.

### Credits and links

Credits are derived from `main`'s Git history.
For each fragment and highlight, the `changelog` crate inspects the commits that
added or modified the file at the root of `changelog.d/` (following renames) up
to the commit that moved it into `changelog.d/<version>/`.
Edits made after the file is moved into a version directory do not add credits.
Each commit's change request is extracted from its commit message, or from its
merge commit message when merged via a merge commit.
If a fragment lists `Credits` fields, those override history lookup; `pick` uses
this mechanism when copying a fragment into a patch release so the copy retains
the original credits, including commit credits.

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

1. **Cut**: A maintainer runs `jp release cut` when `due` succeeds (or on
   demand), pushes the two branches it created, and opens the notes branch's
   pull request.
2. **Prepare**: Contributors edit the release notes in `changelog.d/<version>/`
   on the notes branch and cherry-pick any additional fixes onto
   `release/<major>.<minor>` with `jp release pick`, then merge the notes
   branch.
3. **Finalize**: Once the notes branch has merged and CI passes on the release
   branch, a maintainer runs `jp release finalize` from `main` to tag the
   release, and pushes the tag.

For a patch release, a maintainer runs `jp release pick` targeting the next
patch version, followed by `jp release finalize`.

### JP's CI

Every GitHub Actions workflow invokes a `just` recipe so the same steps can run
locally without GitHub, and no workflow pushes commits to any branch:

| Trigger                       | Runs                                                                                                 | Side effects                                                               |
| ----------------------------- | ---------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| Pull request                  | Changelog fragment check and format validation ([RFD 121])                                       | None                                                                       |
| Push to `main` or `release/*` | Test and lint suite (`just changelog-ci` runs only on `main`, since release branches lack fragments) | None                                                                       |
| Daily schedule                | `just release-due`                                                                                   | Opens an issue labelled `release` if a release is due and none exists      |
| Push to `main`                | Documentation site build ([On the site](#on-the-site))                                               | Deploys the site                                                           |
| Push of a `v*` tag            | Binary distribution ([RFD 123])                                                                  | Publishes `jp` binaries and the GitHub Release                             |
| Push of a `vX.Y.0` tag        | Release reminder cleanup (a minor or major release answers the reminder)                             | Closes the `release` issue                                                 |

Because maintainers push branches and tags themselves, downstream workflows
trigger normally.
In GitHub Actions, pushes and pull requests created with `GITHUB_TOKEN` do not
trigger other workflows and require manual approval to run checks ([Triggering a
workflow from a workflow]); because CI never pushes branches or opens pull
requests, the pipeline needs no bot account or token workarounds.

### On the site

Releases are a third content workflow in `jp vite` ([RFD 116]) alongside tickets
and RFDs, allowing the documentation site to build without compiling `jp`.
`jp vite sync` writes `fragments_dir`, `forge`, and `headings` into the site's
settings snapshot alongside the other plugins' settings.
During the site build, the releases loader runs `changelog render --format json`
for each `changelog.d/<version>/` directory and for the root of `changelog.d/`,
passing the snapshotted settings as CLI flags.
From that JSON model, `jp vite` generates release notes and upgrade guide pages
for each release plus an "Unreleased" page.
A release in preparation appears on the site (marked as in preparation) as soon
as its notes branch merges to `main`, serving as a live preview while notes are
edited.
The generated pages are not committed to Git.
`jp vite` serves them under `/releases/` with a release index showing dates, and
replaces the "Change Log" navigation link with "Releases".

The docs build compiles and installs the `changelog` crate binary in place of
`jilu`, without compiling the rest of the workspace.
The docs workflow already fetches full Git history, which credit resolution
requires.

Release pages are always built from `main`.
Because release tags do not contain `changelog.d/` and `jp vite` only enables a
workflow when its directory exists ([RFD 116]), building the site from a release
tag would omit all release pages and must not overwrite the deployment from
`main`.
JP's docs workflow therefore stops deploying on tag pushes.
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

- **Maintaining custom release-notes tooling**: JP replaces `jilu` with the
  in-tree `changelog` crate and `jp-release` plugin.
  Although the implementation is small (parsing, credit lookup, JSON model, and
  Markdown rendering), the project owns its maintenance.
- **Two-step `cut` and `pick` updates**: The release branch can be pushed
  immediately, whereas moving fragments on `main` waits for the notes branch to
  merge.
  Until it merges, those fragments stay at the root of `changelog.d/`.
- **Publishing is a separate manual step**: The commands push nothing, so a cut
  that nobody pushed blocks the next cut until it is pushed or deleted.
- **`pick` merges `main` into the notes branch**: If the notes branch was edited
  where `main` also changed, `pick` stops until someone merges `main` into the
  notes branch by hand.
- **Branch and directory accumulation**: A two-week cadence creates 26
  `release/<major>.<minor>` branches and 26 `changelog.d/<version>/` directories
  per year.
- **Release tags are not ancestors of `main`**: Because tags sit on
  `release/<major>.<minor>` after the commit that deletes `changelog.d/`, `git
  describe` on `main` does not find release tags.
  The cut point on `main` is the parent of the first commit on the release
  branch.
- **CI compiles `jp` and `jp-release` from source**: Until binary distribution
  is implemented, workflows that invoke `jp release` (primarily the daily `due`
  check) must compile `jp` and `jp-release`.
- **Credit extraction depends on commit message conventions**: Projects or
  forges that do not include change request references in merge or squash
  commits fall back to per-commit credits.

## Alternatives

- **A standalone binary invoked via `just` instead of a command plugin**: Avoids
  the plugin protocol, but separates release commands from the [RFD 116] plugin
  suite used by other workspaces.
- **Opening release pull requests against the release branch and syncing notes
  back to `main`**: Causes release notes to exist on two branches at once,
  creating ambiguity about where to edit historical notes.
- **Pushing the `cut` commit directly to `main`**: Eliminates the change
  request, but requires granting maintainers a branch-protection bypass on
  `main` and lands unreviewed commits directly on the default branch.
  Branching the release branch and the notes branch from the same commit
  achieves the same atomicity without bypassing branch protection.
- **Orphan release branches**: Prevents accidentally merging a release branch
  back into `main` (which would delete `changelog.d/`), but breaks `git bisect`
  across releases, `git log v0.3.0..v0.4.0`, and Git ancestry links between tags
  and `main`.
- **Tagging `main` directly without release branches**: Avoids release branches,
  but makes cherry-picking fixes into a release or shipping a patch release
  without everything currently on `main` impossible.
- **Preparing releases on a short-lived branch that merges back into `main`**:
  Any commit merged to `main` while notes are being prepared is automatically
  included in the release unless merges to `main` are held back.
- **Freezing `main` during release preparation** (as in Vector's release
  checklist): Blocks all other development while release notes are edited.
- **Commands that push and open the pull request through the forge API**: Saves
  a step, but `jp release` would need a token and per-forge code, and a `cut`
  whose second push failed would be left half done with nothing to finish it.
  Rehearsing a release would also require a stand-in remote.
  The forge already responds to a push with the URL for opening the pull request.
- **A separate pull request per pick**, from its own branch off `main`: Avoids
  merging into an open notes branch, but adds a pull request per pick and
  spreads a release's notes across several of them.
- **Automating `cut` or pull request creation in GitHub Actions**: Requires a
  GitHub App token (or manual workflow approvals) for CI-created pushes and pull
  requests to trigger checks, and couples the release workflow to GitHub.
- **Failing CI on `main` when a release is overdue**: Turns unrelated pull
  requests red for a condition their authors cannot fix in their branch.
- **Generating and committing `overview.md` at `cut` time**: Subsequent `pick`s
  invalidate the summary counts, and auto-regenerating the file only when
  unedited introduces hidden state.
- **Committing rendered Markdown release pages**: Removes the `changelog` binary
  from the site build, but freezes rendered output at release time so formatting
  improvements or typo fixes require editing generated files.
- **Deleting fragments after a release**: Keeps `changelog.d/` small, but has
  the same drawback of freezing rendered pages once released.
- **Built-in Cargo version bumping**: Couples `cut` to Rust workspaces, whereas
  a configurable `bump` shell command supports any ecosystem using existing
  tools.
- **Reading the current version from `Cargo.toml`**: Git tags are the single
  source of truth for released versions; writing `Cargo.toml` via `bump` without
  reading it back prevents drift between tags and manifests.

## Non-Goals

- **Building, signing, and distributing binaries**: Covered by separate RFDs
  triggered on tag push ([RFD 123] for the `jp` binary, followed by a separate
  RFD for command plugins including `jp-release`).
  Until those land, JP's CI workflows compile `jp` and `jp-release` from source.
- **Pre-release versions**: Release candidates (`-rc.1`) must not consume
  unreleased fragments needed by the final release and require a separate
  design.
- **Independent per-crate versioning**: All released workspace crates share a
  single version number.
- **Reusable CI workflows for external workspaces**: JP's GitHub Actions
  workflows are specific to JP; `jp release` itself requires no CI integration.
- **Publishing to crates.io**: JP is distributed as prebuilt binaries.

## Risks and Open Questions

- **Resolving plugin configuration without `jp`**: JP's CI workflows must build
  `jp` because only the host binary resolves configuration, whereas the docs
  site build avoids `jp` by reading `.jp/vite.json`.
  A future RFD can add standalone configuration resolution to plugins; because
  the `changelog` crate takes all settings as CLI arguments, only the plugin
  wrapper would change.
  Once [RFD 123] ships prebuilt `jp` binaries, CI can download `jp` instead of
  compiling it.
- **Cherry-picking a breaking change into a non-breaking release**: `pick`
  rejects fragments with an upgrade section when targeting a patch release (or a
  minor release from `1.0` onward), and `cut --version` rejects such a minor
  release.
  If a breaking fix must ship, maintainers must abandon the in-preparation
  release and cut a major release instead.
- **Abandoning a release in preparation**: There is no dedicated `abandon`
  subcommand.
  Abandoning an unfinalized release requires moving its fragments back to the
  root of `changelog.d/` on `main` and deleting the `release/<major>.<minor>`
  branch.
  A dedicated command can be added if this becomes common.
- **`Cargo.lock` merge conflicts**: The notes branch updates `Cargo.lock` via
  `bump` and stays open while notes are prepared, so a change to `Cargo.lock`
  merged to `main` in the meantime may conflict with it.
  Merging `main` into the notes branch and re-running `bump` resolves the
  conflict.
- **Highlights modified across a release cut**: Under [RFD 121], multiple pull
  requests contributing to a feature edit the same highlight file.
  If a pull request merges after the cut point and extends a highlight that the
  notes branch is moving into `changelog.d/<version>/`, Git's rename
  merge carries those additions into the released highlight even though that
  pull request's code is not on the release branch.
  Reviewers should check highlights during release preparation and move any
  post-cut additions back to `main`.

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

Implement `jp release due`, add the `just release-due` recipe, and add the daily
GitHub Actions workflow that opens a `release`-labelled reminder issue (closed
automatically when the tag of the next minor or major release is pushed).
Depends on Phase 2.

### Repository settings

No repository settings block Phase 2: `main`'s branch protection already accepts
the notes branches' pull requests, and no rules currently restrict `release/*`,
so release branches can be pushed.
Once Phase 2 merges, a repository administrator should configure branch and tag
protection to prevent history rewrites:

- On `release/*`: block force pushes and branch deletion.
- On `v*` tags: block tag updates and deletion.

### Phase 4: Documentation site and retiring `jilu`

Add the releases workflow to `jp vite`: snapshot `fragments_dir`, `forge`, and
`headings` in `jp vite sync`, add the releases loader, generate the release
notes, upgrade guide, index, and "Unreleased" pages, and replace "Change Log"
with "Releases" in the site navigation.
Update the docs workflow to build `changelog` instead of `jilu`, stop deploying
the site on tag pushes, and remove `jilu` and `docs/change-log.md`.
Depends on Phase 1 and the site phase of [RFD 116].

## References

- [RFD 116: Shareable RFD, Ticket, and Site Tooling][RFD 116]: the command
  plugin architecture and `jp vite` workflow model extended by this RFD.
- [RFD 121: Changelog Fragments and Validation][RFD 121]: the fragment format
  and `jp release` plugin that this pipeline builds on.
- [RFD 001: The JP RFD Process][RFD 001]: the RFD lifecycle followed by this
  document.
- [Bevy release news example], [Bevy migration guide example], and [bevy-website
  `generate-release`]: prior art for separating release notes from upgrade
  guides and assembling both from fragments.
- [Vector releases index] and [Vector's release checklist]: prior art for
  per-release pages and the `main`-branch freeze avoided by this design.
- [Triggering a workflow from a workflow]: GitHub Actions rules on events
  created by `GITHUB_TOKEN`.
- [GitLab commit message templates]: GitLab's merge and squash commit formats.
- [cargo-edit]: provides `cargo set-version`, used as JP's `bump` command.

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
