# RFD 122: Release Pipeline

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-01
- **Extends**: [RFD 116]
- **Requires**: [RFD 121]
- **Required by**: [RFD 123]
- **Summary**: The `jp release` plugin cuts releases onto branches, files their
  notes on `main` through pull requests, and tags them.

## Summary

This RFD extends the `jp release` plugin ([RFD 121]) with commands that cut,
patch, and tag releases while keeping all release notes and upgrade guides on
`main`.
`jp release cut` branches `release/<series>` for code and opens a pull request
moving unreleased fragments into the release's directory on `main`.
`jp release pick` cherry-picks fixes onto the release branch, and `jp release
finalize` tags the release.
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

## Design

### Repository layout

| Location           | Contents                                                                                                                                |
| ------------------ | --------------------------------------------------------------------------------------------------------------------------------------- |
| `main`             | Active development and all release notes: unreleased fragments at the root of `changelog.d/`, released ones in `changelog.d/<version>/` |
| `release/0.3`      | The code shipped in `0.3.x` releases: `main` at the cut point, without `changelog.d/`, plus cherry-picked commits                       |
| `v0.3.0`, `v0.3.1` | Annotated tags on `release/0.3` marking finalized releases                                                                              |

No separate state file is used:

- A release **ships** a change when its branch contains the commit that
  introduced it, either from `main` at the cut point or via `jp release pick`.
- A fragment in `changelog.d/<version>/` belongs to that release.
  A fragment at the root of `changelog.d/` is **unreleased** until a release
  branch contains its commit.
  `cut` or `pick` opens a change request moving the fragment into
  `changelog.d/<version>/`; until that change request merges, the fragment still
  renders as unreleased.
- A release is **in preparation** from the initial commit on its release branch
  until it is tagged.
  Tagging produces a **finalized** release.

### Commands

[RFD 121] introduces the plugin with `init`, `add`, and `check`.
This RFD adds:

| Command                              | Description                                                                                      |
| ------------------------------------ | ------------------------------------------------------------------------------------------------ |
| `jp release cut [--version VERSION]` | Starts the next minor or major release from `main`                                               |
| `jp release pick VERSION COMMIT...`  | Cherry-picks commits from `main` onto a release in preparation, or starts the next patch release |
| `jp release finalize VERSION`        | Tags a finalized release                                                                         |
| `jp release render [VERSION]`        | Renders a release's notes or upgrade guide, or the unreleased fragments                          |
| `jp release due`                     | Exits zero when the next release is due                                                          |
| `jp release review [VERSION]`        | Asks the assistant to review a release's notes and upgrade guide                                 |

`cut`, `pick`, and `finalize` operate in a temporary worktree based on
`origin`'s branches, leaving the caller's working tree untouched.
They push using the caller's credentials: release branches and tags are pushed
directly, while updates to `main` are pushed to a branch and submitted through a
change request ([Changes to `main`](#changes-to-main)).
Following [RFD 116], none of these commands are exposed as LLM tools because
they access the network or make project-level release decisions.

### Configuration

```toml
[plugins.command.release.options]
cadence = "2w"
forge = "github"
bump = "cargo set-version --package jp_cli {version}"
change_request_labels = ["no-changelog"]

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
  links](#credits-and-links)) and how `cut` and `pick` open change requests
  ([Changes to `main`](#changes-to-main)).
  When unset, it is inferred from the `origin` remote URL: `github` for
  `github.com`, `gitlab` for `gitlab.com`, and `none` for any other host.
  Self-hosted GitHub or GitLab instances set `forge` explicitly and use the
  remote's hostname for links.
- `bump` has no default.
  When unset, the version is recorded only in the Git tag.
- `change_request_labels` lists the forge labels applied to change requests
  opened by `cut` and `pick` (empty by default).
  JP sets `["no-changelog"]` to satisfy [RFD 121]'s CI check, since release
  change requests move existing fragments rather than adding new ones.
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
version in the initial commit on the release branch, and again in the `cut`
change request so `main` records the latest cut version.
This lets projects update version files wherever they keep them using their
existing tools.
JP's `bump` command runs `cargo set-version` (from `cargo-edit`) on `jp_cli`,
which updates `[workspace.package] version`, every crate that inherits it, and
`Cargo.lock` (checked in CI with `--locked`), while leaving the independently
versioned crates under `crates/contrib/` untouched.
Passing `--workspace` would overwrite those `crates/contrib/` versions as well.

### Cutting a release

`jp release cut`:

1. Rejects shallow clones (which lack the commit history needed for credits) and
   refuses to run when `main` has no unreleased fragments.
2. Computes the next version at the tip of `origin/main` (the cut point).
3. Creates `release/<major>.<minor>` at that commit, adds a commit that deletes
   `changelog.d/` and runs `bump`, and pushes the branch.
4. Creates `release-cut/v<version>` from the same commit, moves (`git mv`) every
   unreleased fragment and highlight into `changelog.d/<version>/`, runs `bump`,
   commits, pushes the branch, and opens a change request against `main`.

<!-- end list -->

```
changelog.d/
  README.md
  enable-explicit.md          # merged after the cut: unreleased
  0.3.0/
    retry-idle-turns.md
    highlights/plugin-tools.md
```

Because both branches start from the same commit, the code on the release branch
and the fragments moved into `changelog.d/<version>/` match regardless of how
long the change request takes to merge.
Any fragment merged to `main` after the cut point remains at the root of
`changelog.d/`, and its code is not on the release branch, so both belong to the
next release.
If a correction to a moved fragment merges on `main` before the cut's change
request merges, Git's rename tracking carries that edit into
`changelog.d/<version>/`.
Because [RFD 121] requires every new change to add a new fragment file, any edit
to an existing fragment is a correction and belongs in the release that ships
that fragment.

Whether a fragment is unreleased depends on what release branches contain its
commit, not on its current directory path.
If a fragment is still at the root of `changelog.d/` only because an earlier
`cut` or `pick` change request has not merged yet, its commit is already on a
release branch, so a subsequent `cut` leaves it for that pending change request.
`cut` and `pick` never block on earlier change requests.

Because the release branch deletes `changelog.d/`, release notes cannot be
edited on the release branch.
The version-bump commits that start releases are the only commits on a release
branch that do not originate on `main`.

### Changes to `main`

`cut` and `pick` push to the release branch directly, but update `main` through
a branch and a change request like any other change:

- `github` opens a pull request via the `jp_github` client crate (extended to
  create and label pull requests) using JP's standard GitHub authentication,
  without shelling out to `gh`.
- `gitlab` opens a merge request using Git push options (`merge_request.create`,
  `merge_request.target`, `merge_request.title`, and `merge_request.label`),
  requiring only `git` and push credentials.
- `none` pushes the branch and stops, leaving the merge to the project's
  workflow.

Each change request applies `change_request_labels`.
Because the caller opens the change request under their own account, CI checks,
review, and merging work the same way as on any contributor change request.

### Preparing a release

Preparing release notes takes as long as needed without blocking `main`:

- **Editing notes**: Fragment files, highlights, and the overview in
  `changelog.d/<version>/` are edited on `main` through normal change requests.
  To combine two fragments for a single logical change (such as a feature and a
  follow-up fix), edit one fragment, delete the other, and list both change
  requests in the surviving fragment's `change_requests` frontmatter.
- **Overview**: The overview introduces the release notes.
  When `changelog.d/<version>/overview.md` is absent, rendering generates a
  default summary stating the version, the count of entries per type, and the
  highlight titles.
  Adding `overview.md` replaces the generated summary; `jp release render
  <version>` prints the generated text as a starting point.
- **Cherry-picking commits**: `jp release pick 0.3.0 <commit>...` cherry-picks
  the specified commits onto `release/0.3` (`git cherry-pick -x`), strips any
  changes under `changelog.d/`, and pushes the release branch.
  On a new branch `release-pick/v0.3.0-<short-sha>` based on `main`, it places
  the fragments and highlights introduced by those commits into
  `changelog.d/0.3.0/` and opens a change request.
  If no later release ships the fragment, `pick` moves it so the next release
  does not list it again.
  If a later release was already cut after the commit merged to `main`, `pick`
  copies the fragment and writes its resolved credits into the copy's
  `change_requests` field ([RFD 121]), so both releases include the change and
  list the entry.
  If cherry-picking hits a merge conflict, `pick` stops and prints the temporary
  worktree path; after resolving the conflict there, run `jp release pick
  --continue`.

Because code reaches a release branch only via `cut` or `pick`, every change
lands on `main` first.

### Finalizing a release

`jp release finalize 0.3.0` creates an annotated `v0.3.0` tag at the tip of
`origin/release/0.3` and pushes the tag.
Maintainers run `finalize` once checks pass on the release branch (via GitHub CI
on `release/*` pushes, or `just ci` locally).

Because a maintainer pushes the tag directly, the push triggers JP's binary
distribution workflow ([RFD 123]).
Release notes remain editable on `main` after tagging: updating files in
`changelog.d/<version>/` re-renders the notes wherever they are published,
including when a `cut` or `pick` change request merges after the tag is pushed.

### Patch releases

Patch releases branch from their release series branch rather than `main`.
After `v0.3.0` is tagged, `jp release pick 0.3.1 <commit>...` starts `0.3.1` by
adding a version-bump commit (`bump`) on `release/0.3`, cherry-picking the
requested commits, and opening a change request on `main` that moves or copies
their fragments into `changelog.d/0.3.1/`.
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
while `main` cuts `0.4.0`, since each uses its own release branch and its own
`changelog.d/<version>/` directory on `main`.

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
If a fragment defines an explicit `change_requests` list in its frontmatter,
that list overrides history lookup; `pick` uses this mechanism when copying a
fragment into a patch release so the copy retains the original credits.

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
GitLab projects that squash-merge receive commit credits unless they include
`%{reference}` in their squash commit template.

Because credit resolution requires Git history, `render` rejects shallow clones
rather than emitting incomplete credits.

### Release workflow

`jp release due` exits zero when `cadence` has elapsed since the latest minor or
major release was cut and that release has been finalized.

1. **Cut**: A maintainer runs `jp release cut` when `due` succeeds (or on
   demand).
2. **Prepare**: Contributors edit the release notes in `changelog.d/<version>/`
   on `main` and cherry-pick any additional fixes onto `release/<major>.<minor>`
   with `jp release pick`.
3. **Finalize**: Once CI passes on the release branch, a maintainer runs `jp
   release finalize` to tag and publish the release.

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

Because maintainers push branches and tags from their own machines, downstream
workflows trigger normally.
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
as its `cut` change request merges to `main`, serving as a live preview while
notes are edited.
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

### `review`

`jp release review` sends a structured `query` request (protocol 11) to the host
`jp` process, using the same mechanism as `jp rfd track` ([RFD 116]).
It asks the assistant to flag entries that read like raw commit messages,
entries that fail to explain the user-facing impact, upgrade sections that omit
migration steps, and typos.
`review` prints findings to the terminal without modifying files; it is an
advisory check, not a CI gate.

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
- **Two-step `cut` and `pick` updates**: Pushing the release branch is
  immediate, whereas moving fragments on `main` waits for the change request to
  merge.
  Until that change request merges, those fragments continue rendering as
  unreleased.
- **Multi-branch state during `pick` conflicts**: `pick` updates both the
  release branch and a change-request branch for `main`; a cherry-pick conflict
  pauses the command midway until the user resolves the conflict and runs
  `--continue`.
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
  the plugin protocol, but cannot request host LLM queries for `jp release
  review` and separates release commands from the [RFD 116] plugin suite used by
  other workspaces.
- **Opening release pull requests against the release branch and syncing notes
  back to `main`**: Causes release notes to exist on two branches at once,
  creating ambiguity about where to edit historical notes.
- **Pushing the `cut` commit directly to `main`**: Eliminates the change
  request, but requires granting maintainers a branch-protection bypass on
  `main` and lands unreviewed commits directly on the default branch.
  Branching the release branch and the `cut` change request from the same commit
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
- **`Cargo.lock` merge conflicts**: Because the `cut` change request updates
  `Cargo.lock` via `bump`, concurrent pull requests that touch `Cargo.lock` may
  conflict with it.
  Rebasing the `cut` branch and re-running `bump` resolves the conflict.
- **Highlights modified across a release cut**: Under [RFD 121], multiple pull
  requests contributing to a feature edit the same highlight file.
  If a pull request merges after the cut point and extends a highlight that the
  `cut` change request is moving into `changelog.d/<version>/`, Git's rename
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
release branch creation, change request creation on `main` across all three
forge modes (extending `jp_github` with pull-request creation and labelling, and
using GitLab push options), `pick` (including fragment moves and copies), patch
releases, and `finalize`.
Configure `bump` and `change_request_labels` for JP, and enable CI checks on
pushes to `release/*`.
Depends on Phase 1.

### Phase 3: `due` and the reminder workflow

Implement `jp release due`, add the `just release-due` recipe, and add the daily
GitHub Actions workflow that opens a `release`-labelled reminder issue (closed
automatically when the tag of the next minor or major release is pushed).
Depends on Phase 2.

### Repository settings

No repository settings block Phase 2: `main`'s branch protection already accepts
the change requests opened by `cut` and `pick`, and no rules currently restrict
`release/*`.
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

### Phase 5: `review`

Implement `jp release review` using host LLM queries.
Depends on Phase 1.

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
- [GitLab commit message templates] and [GitLab push options]: GitLab's merge
  commit format and push options for opening merge requests.
- [cargo-edit]: provides `cargo set-version`, used as JP's `bump` command.

[Bevy migration guide example]: https://bevy.org/learn/migration-guides/0-18-to-0-19/
[Bevy release news example]: https://bevy.org/news/bevy-0-19/
[GitLab commit message templates]: https://docs.gitlab.com/user/project/merge_requests/commit_templates/
[GitLab push options]: https://docs.gitlab.com/topics/git/commit/#push-options-for-merge-requests
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
