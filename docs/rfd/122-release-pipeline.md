# RFD 122: Release Pipeline

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-01
- **Extends**: [RFD 116]
- **Requires**: [RFD 121]
- **Summary**: The `jp release` plugin cuts releases onto branches, files their
  notes on `main` through pull requests, and tags them.

## Summary

This RFD adds the release pipeline to the `jp release` plugin of [RFD 121].
`jp release cut` branches `release/0.3` from `main` to hold the code 0.3.x
ships, and opens a pull request that moves every unreleased fragment into the
release's directory on `main`.
Release notes are written and corrected on `main` alone; release branches carry
code and no notes, and `jp release pick` adds code to them by cherry-picking
from `main`.
`jp release finalize` tags a release.
Each release gets release notes and, when it breaks something, an upgrade guide,
rendered from its directory whenever they are needed.
Every step is a command a person runs, and every change to `main` is a pull
request like any other; a project without a forge merges those branches however
it merges.
JP's CI adds a reminder, checks, and distribution, and writes to no branch.
It retires `jilu`; building, signing, and distributing binaries are separate
RFDs.

## Motivation

A release has to be automated and smooth enough that cutting one does not feel
like an event.
JP releases every two weeks at first: development moves fast, and the releases
serve the people building JP more than users who would have to keep up with
them.
The cadence is a setting, so it can slow down once JP has such users.

[RFD 121] gives the raw material, per-change fragments and per-feature
highlights recorded in each pull request; this RFD turns that material into
published releases.
Each release has two documents, as Bevy's do: release notes to read, and an
upgrade guide to work through when something breaks.
Keeping upgrade steps out of the notes lets them take the room they need without
burying the story.

Preparing a release's notes can take days, and nobody else should wait for it.
So a release is cut onto its own branch: what it ships is fixed when it is cut,
`main` keeps moving, and anything that should still ship is picked over
deliberately.

The workflow has one path.
Every artifact has one home and every step one command: release notes live on
`main` alone, so correcting any release's notes has one answer; release branches
have no notes, so there is nothing on them to correct; and changes reach `main`
the way every change does, so `main` needs no exception for releases.
The design gets there by construction rather than by checks that catch the other
paths.

The plugin serves other JP workspaces alongside [RFD 116]'s, and not every
contributor uses a forge's web interface.
So every step is a command, a forge is only where changes are reviewed, and a
project without one runs every step with git alone.
JP's CI is a convenience around the commands, not a requirement.

## Design

### Where things live

| Where              | Holds                                                                                                                                      |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------ |
| `main`             | Development, and every release's notes: unreleased fragments at the top level of `changelog.d/`, each release's in its version's directory |
| `release/0.3`      | The code 0.3.x releases ship: `main` at the cut, without `changelog.d/`, plus the commits picked onto it                                   |
| `v0.3.0`, `v0.3.1` | Tags on `release/0.3`: the releases                                                                                                        |

Nothing else records state:

- A release **ships** a change when its branch contains the commit that made it,
  from `main`'s history at the cut or through a pick.
- A fragment in a version's directory belongs to that release.
  One at the top level is **unreleased** until a release ships its change; the
  change request that `cut` or `pick` opens then moves it, and until that merges
  it still shows as unreleased.
- A release is **in preparation** from the commit that starts it on its branch
  until its tag, and a tag is a **finalized** release.

### Commands

[RFD 121] creates the plugin with `init`, `add`, and `check`.
This RFD adds:

| Command                              | Does                                                                                             |
| ------------------------------------ | ------------------------------------------------------------------------------------------------ |
| `jp release cut [--version VERSION]` | Starts the next minor or major release from `main`                                               |
| `jp release pick VERSION COMMIT...`  | Adds commits from `main` to a release in preparation, or starts the next patch release with them |
| `jp release finalize VERSION`        | Tags a release                                                                                   |
| `jp release render [VERSION]`        | Prints a release's notes or upgrade guide, or the unreleased fragments                           |
| `jp release due`                     | Succeeds when the next release should be cut                                                     |
| `jp release review [VERSION]`        | Asks the assistant to review a release's notes and upgrade guide                                 |

`cut`, `pick`, and `finalize` work in a temporary worktree based on `origin`'s
branches, so the checkout they run from is left alone.
They push as the person running them: release branches and tags directly, and
changes to `main` on a branch of their own, through a change request ([Changes
to `main`](#changes-to-main)).
None is served as a tool: under [RFD 116]'s rule, commands that reach the
network or decide something for the project are the user's to run.

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

These are JP's values, and every option is optional:

- `cadence` defaults to two weeks, and takes a duration as `jp query --tmp`
  does.
- `forge` is `github`, `gitlab`, or `none`, and decides how releases credit and
  link change requests ([Credits and links](#credits-and-links)) and how `cut`
  and `pick` open theirs ([Changes to `main`](#changes-to-main)).
  Unset, it follows git's `origin` remote: `github` for github.com, `gitlab` for
  gitlab.com, and `none` for any other host.
  A self-hosted GitHub or GitLab sets it, and links use the remote's host.
- `bump` has no default.
  Without it, a version lives only in its tag.
- `change_request_labels` lists the forge labels `cut` and `pick` give the
  change requests they open, and defaults to none.
  JP's lists `no-changelog`, which passes [RFD 121]'s gate: those change
  requests move fragments rather than add them.
- `headings` renames a type's section in the release notes.
  A type it does not name keeps its default heading, its id capitalized
  (`Added`, `Performance`), and layered `headings` merge key by key.

`fragments_dir` belongs to [RFD 121].

### Versions

Git tags are the record of what was released: the current version is the highest
`v`-prefixed semantic-version tag, and with none, the first release is `0.1.0`.
Tags are annotated, and a release's date is its tag's date.

A cut from `main` is a minor release, or from `1.0` on a major one when an
unreleased fragment has an upgrade section; `--version` overrides.
Its version follows the highest of the tags and the releases in preparation, so
a cut while another release is in preparation cuts the one after it.
A patch release comes only from a release branch, through `pick` ([Patch
releases](#patch-releases)).

JP tagged `v0.1.0` before this pipeline existed and published no release for it,
so its first release is `v0.2.0`, covering what merged after [RFD 121]'s gate
landed.
A release meant to introduce JP to new readers says so in its overview and
highlights; the pipeline treats every release alike.

`bump` runs with `{version}` replaced by the new version in the commit that
starts a release on its branch, and for a cut also in its change request, so
`main` carries the version it was last cut at.
So a project writes its version wherever it keeps one, with a tool it already
has.
JP's command, `cargo set-version` from cargo-edit, sets `[workspace.package]
version` and every crate that inherits it, leaves the crates under
`crates/contrib/` with their own versions alone, and updates `Cargo.lock`, which
JP's CI checks with `--locked`.
It names `jp_cli`, one of the crates that inherit the workspace version, because
`--workspace` would also overwrite the crates that keep their own.

### Cutting a release

`jp release cut`:

1. Refuses a shallow clone, which lacks the history credits come from, and a
   `main` with no unreleased fragments, which has nothing to release.
2. Computes the version, at the tip of `origin`'s `main`: the commit the release
   is cut at.
3. Starts the release branch: creates `release/<major>.<minor>` at that commit,
   adds one commit that deletes `changelog.d/` and runs `bump`, and pushes it.
4. Files the cut on `main`: on a branch `release-cut/v<version>` from the same
   commit, moves (`git mv`) every unreleased fragment and highlight into
   `changelog.d/<version>/`, runs `bump`, commits, pushes the branch, and opens
   its change request.

<!-- end list -->

```
changelog.d/
  README.md
  enable-explicit.md          # merged after the cut: unreleased
  0.3.0/
    retry-idle-turns.md
    highlights/plugin-tools.md
```

Both branches start at the same commit, so the code the release ships and the
fragments it lists are chosen together, however long the change request takes to
merge.
A fragment merged meanwhile stays at the top level, since the change request
moves only the ones it found, and its code is not on the release branch: it
belongs to the next release on both counts.
A correction merged meanwhile to a fragment the cut moves is carried into the
release's directory when the change request merges, since git follows the
rename; under [RFD 121]'s rule that every change adds its own fragment, an edit
to an existing fragment is a correction, and it belongs on the released copy.

"Unreleased" is decided by what release branches ship, not by where a fragment
sits: a fragment still at the top level because another cut's or pick's change
request has not merged yet is already shipped, and `cut` leaves it for that
change request.
Cuts and picks never wait for each other's change requests.

The release branch has no `changelog.d/`, so notes cannot be written there:
there is nowhere to write them.
The commits that start releases on it, which bump the version, are the only
commits on it that do not come from `main`.

### Changes to `main`

`cut` and `pick` push the release branch directly and change `main` through a
branch of their own and a change request, the way any change reaches `main`:

- `github` opens a pull request through the `jp_github` client crate, extended
  with pull-request create and label calls, and authenticates the way JP's other
  GitHub features do.
  It never runs `gh`.
- `gitlab` opens a merge request with git push options (`merge_request.create`,
  `merge_request.target`, `merge_request.title`, and `merge_request.label`), so
  it needs nothing beyond `git` and the credentials that push.
- `none` stops after pushing the branch, for the project to merge however it
  merges.

Each change request carries `change_request_labels`.
The person running the command opens it, so its checks run as on any change
request they open, and it is reviewed and merged like one.

### Preparing a release

A release in preparation takes as long as its notes need, and nobody waits for
it:

- **Notes** are changes on `main`, made the way any change is made: the wording
  of fragments and highlights in the release's directory, and its overview.
  Two entries for one change, such as a change and its follow-up, become one by
  editing one, deleting the other, and listing both change requests in the kept
  fragment's `change_requests`.
- **The overview** opens the release notes.
  Without an `overview.md` in the release's directory, it is a summary generated
  at render: the version, how many entries of each type the release has, and its
  highlights' titles.
  Writing `overview.md` replaces it, and `jp release render <version>` prints
  the summary to start from.
- **More code** comes from `jp release pick 0.3.0 <commit>...`.
  It cherry-picks the commits onto `release/0.3` (`git cherry-pick -x`), leaving
  out what they change under `changelog.d/`, and pushes it.
  On a branch `release-pick/v0.3.0-<short-sha>` from `main`, it puts each
  fragment and highlight the commits added into `changelog.d/0.3.0/`, and opens
  its change request.
  A fragment no other release ships moves, so the next release does not list the
  change a second time.
  One that a later release already ships, because that release was cut after the
  commit merged, is copied, with its credits written into the copy's
  `change_requests` ([RFD 121]): both releases ship the change, and both list
  it.
  A conflict stops `pick` in its worktree, which it names; resolve the conflict
  there and run `jp release pick --continue`.

Code reaches a release branch only through `pick`, so every change lands on
`main` first.

### Finalizing a release

`jp release finalize 0.3.0` tags the tip of `origin`'s `release/0.3` with an
annotated `v0.3.0` and pushes the tag.
That is the release.
The person running it decides when: on GitHub, CI runs on every push to a
release branch and shows on its tip; elsewhere, `just ci` runs the same checks
locally.

The tag push is a person's, so it starts JP's distribution workflow, which the
binary-distribution RFD defines.
The release's notes stay on `main`: correcting them, at any time, is a change to
its directory there, and every place the notes appear renders them again.
A cut's or pick's change request that merges after the tag is one such change.

### Patch releases

A patch release is cut from its line's branch, not from `main`.
After `v0.3.0`, `jp release pick 0.3.1 <commit>...` starts 0.3.1 with a commit
on `release/0.3` that runs `bump`, then picks the commits as above, moving or
copying their fragments into `changelog.d/0.3.1/`.
A patch release refuses a fragment with an upgrade section.
`jp release finalize 0.3.1` tags it.
`pick` accepts a release in preparation, or the next patch release of a line
whose last release is tagged, which is the version any other number would
collide with.

### Releases in parallel

Each release branch holds one release in preparation at a time: a patch release
starts after the previous release on its line is tagged.
Releases on different branches do not touch each other.
`release/0.3` can prepare 0.3.1 while `main` cuts 0.4.0, since each writes its
own branch and its own version's directory on `main`.

### Release notes and the upgrade guide

Each release has two documents:

- **Release notes**: the overview, the highlights, and every fragment grouped by
  type under its heading.
  A breaking entry is marked and links to its section of the upgrade guide.
- **The upgrade guide**: one section per fragment with an upgrade section ([RFD
  121]), headed by its title and followed by the entry's credits and the
  section's body.
  A release without upgrade sections has no upgrade guide.

Release notes, as `jp release render 0.3.0` prints them.
The entries are paraphrased from recent pull requests; the breaking change and
the last entry are invented:

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

The same release's upgrade guide, as `jp release render 0.3.0 --page upgrade`
prints it:

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

Neither document has a title or a date; whoever places them adds both.

### The release model

The `changelog` crate builds a release model from a release's directory, or from
the unreleased top level, and serializes it.
Markdown is one view of that model; a consumer that renders its own way reads
the JSON:

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

- Bodies, titles, and the overview are Markdown strings; the overview is the
  generated summary when the release has no `overview.md`.
- `type` is [RFD 121]'s id, never a heading: a consumer maps ids to the headings
  it shows.
- A credit's `kind` is `change_request` or `commit`, and its `label` is how the
  forge writes it (`#1182` on GitHub, `!1182` on GitLab, a short hash for a
  commit), so a consumer shows the label linked to the URL without knowing which
  forge it came from.
  Under `forge = "none"`, credits are commits with no URL.
- A release in preparation has a `null` date, and the unreleased model a `null`
  version too.
- Within a type, entries are ordered by their first credit and then by slug, so
  the same files always give the same output.
- `format` is the model's version, so a consumer can refuse output it does not
  understand.

`changelog render` (the crate's binary) and `jp release render` take `--format
json` or `--format markdown`, and for Markdown `--page notes` or `--page
upgrade`.
The binary reads no configuration: the fragments directory, the forge, and the
headings arrive as arguments, which `jp release render` fills in from the
plugin's options.
In Markdown, a breaking entry links to its upgrade section at a location the
consumer passes.

Every consumer renders from `main`, where the notes live, and places the
documents itself:

- `jp release render` prints them, to read while a release is prepared or to
  hand to another tool.
- `jp vite` turns them into pages on the site ([On the site](#on-the-site)).
- The binary-distribution RFD puts the release notes in the GitHub Release,
  linking to the upgrade guide on the site, and renders them again when the
  release's directory changes on `main`.

### Credits and links

Credits come from `main`'s history.
For each fragment and highlight, the crate reads the commits that wrote or
changed it while it sat at the top level of `changelog.d/`, following renames,
up to the commit that moved it into a release's directory.
Corrections after that credit nobody.
A commit's change request is read from its message or, when the commit reached
`main` through a merge, from the merge commit's message.
A fragment whose `change_requests` list is set takes that list instead, which is
how a copy made for a patch release keeps the original's credits.

| `forge`  | Change request read from                                                         | Label  | Links to                                        |
| -------- | -------------------------------------------------------------------------------- | ------ | ----------------------------------------------- |
| `github` | `(#123)` ending a squash commit's subject, or `Merge pull request #123`          | `#123` | `https://<host>/<owner>/<repo>/pull/123`        |
| `gitlab` | `See merge request <project>!123`, as GitLab's default merge commit message ends | `!123` | `https://<host>/<project>/-/merge_requests/123` |
| `none`   | nothing                                                                          |        |                                                 |

A commit with no change request is credited as itself, linked to its page on the
forge: `/commit/<sha>` on GitHub, `/-/commit/<sha>` on GitLab.
JP squash-merges with GitHub's default subject, so its entries credit their pull
requests.
GitLab's default squash commit message is the merge request's title alone, so a
GitLab project that squashes gets commit credits unless it adds `%{reference}`
to its squash template.

Crediting reads history, so rendering refuses a shallow clone rather than
produce documents without their credits.

### The train

`due` succeeds when `cadence` has passed since the newest minor or major release
was cut, and that release has been finalized.

1. **Cut.** A maintainer runs `jp release cut`, when `due` says so or on any
   other day.
2. **Prepare.** They and others edit the release's notes on `main` and pick
   later work onto its branch, for as long as it takes.
3. **Finalize.** They run `jp release finalize` once the release branch's checks
   pass.

A patch release is a `pick` into the next patch version, then `finalize`.

### JP's CI

Every workflow runs a `just` recipe, so each step also runs on a machine without
GitHub, and no workflow writes to a branch:

| When                            | Runs                                                                                            | Writes                                                             |
| ------------------------------- | ----------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| A pull request                  | The changelog gate and format validation ([RFD 121])                                        | Nothing                                                            |
| A push to `main` or `release/*` | The checks; on `main`, `just changelog-ci` among them, since release branches have no fragments | Nothing                                                            |
| Daily                           | `just release-due`                                                                              | An issue labelled `release` when a release is due and none is open |
| A push to `main`                | The site build ([On the site](#on-the-site))                                                    | The site                                                           |
| A push of a `v*` tag            | Distribution (the binary-distribution RFD)                                                      | Release binaries, the GitHub Release; closes the reminder issue    |

Every push and every tag comes from a person running a command, so every
workflow starts on its own.
A push made with a workflow's own token starts no other workflow, and a pull
request it opens waits for a maintainer to approve its checks ([Triggering a
workflow from a workflow]); the pipeline never does either, so it needs no
approval and no bot identity.

### On the site

Releases are a third workflow for `jp vite` ([RFD 116]), next to tickets and
RFDs, and the site builds without `jp`.
`jp vite sync` writes `fragments_dir`, `forge`, and `headings` into its settings
snapshot with the other plugins' settings.
The releases loader runs `changelog render --format json` for each version's
directory and for the unreleased top level, with arguments from the snapshot,
and builds the pages from the model: release notes and an upgrade guide for each
release, and an "Unreleased" page.
A release in preparation has its pages from the moment its cut's change request
merges, marked as such, so they double as the preview while its notes are
prepared.
The pages are never committed.
Where they go is `jp vite`'s choice: JP serves them under `/releases/`, with an
index of releases and their dates, and replaces the navigation's "Change Log"
entry with "Releases".

The docs build installs the `changelog` binary in place of `jilu`: compiled from
the checkout until a release publishes it, downloaded after.
Its checkout already fetches full history, which crediting needs.

Release pages are rendered from `main`, where the notes live.
A release tag's tree has no `changelog.d/`, and `jp vite` enables a workflow
only when its directory exists ([RFD 116]), so a site built from a tag has no
release pages, and must not replace `main`'s.
JP's docs workflow deploys on tag pushes today, and that stops.
A future site with documentation per release stays open: it builds each tag's
tree into a path of its own and links to the release pages `main`'s site serves.

### `review`

`review` asks the host for a structured `query` (protocol 11) about a release's
notes and upgrade guide, as `jp rfd track` does in [RFD 116]: entries that read
like commit messages, entries that do not say what changed for the user, upgrade
sections that do not say what to do, and typos.
It prints what it finds and edits nothing.
It is an opinion, not a gate.

### Other artifacts

Command plugins ship continuously through the rolling `plugins` release, so a
plugin change reaches users before its fragment reaches a release's notes.
Plugin crates inherit the workspace version, but `plugins.yml` does not rebuild
them on a version bump, so the rolling binaries report the previous version
until their next build.
A later RFD on distributing plugins decides whether a release also publishes
versioned plugin binaries.

The macOS app ([RFD 099]) is not distributed and keeps its own version, so it is
outside the release.

### Retiring jilu

Release pages replace the git-history-generated `docs/change-log.md`.
The `build-changelog` recipe, the `jilu` dependency, and the docs-build step
that generates the change log are removed.

## Drawbacks

- **The project owns the assembly logic.** `jilu` is replaced by code JP
  maintains.
  The logic is small (parse, credit, model, render), but it is now ours to test
  and fix.
- **A cut or pick lands in two steps.** The release branch moves at once; `main`
  follows when the change request merges, and until then the release's new
  fragments still show as unreleased.
- **`pick` writes two branches.** A conflict stops it between them, and
  `--continue` finishes the job.
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
  system, but it cannot ask the host for the query `review` needs, and it splits
  release tooling from the plugin family other workspaces install.
- **Release pull requests into the release branch, with the notes synced back to
  `main` afterwards.** The notes would live on both branches, and correcting an
  old release would have two possible places, one of them wrong.
- **Committing the cut to `main` directly.** One step instead of two, but on
  GitHub it needs a bypass of `main`'s pull-request rule for whoever cuts
  releases, which lets them push anything else too, and it puts unreviewed
  commits on `main`.
  Starting the release branch and the change request from the same commit makes
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

- **Building, signing, and distributing binaries.** Separate RFDs, started by
  the tag push: the `jp` binary first, plugins after.
  CI installs the release tooling instead of building it once releases publish
  `jp-release`, the `changelog` binary, and [RFD 116]'s plugins alongside `jp`.
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
  `1.0` on into a minor one.
  Shipping it means abandoning the release in preparation and cutting a major
  one.
- **Abandoning a release in preparation** has no command: move its fragments
  back to the top level on `main` and delete its branch.
  It has never been needed; a command can follow if it is.
- **Lockfile conflicts.** A cut's change request changes `Cargo.lock`, so a pull
  request that changes it too may conflict with it, and running `bump` again on
  the rebased branch resolves it.
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

Add the release model, crediting for all three forges, and the Markdown views to
`crates/internal/changelog`, with `changelog render`, and add `jp release
render` with the `forge` and `headings` options.
Depends on [RFD 121] Phases 2 and 3.

### Phase 2: `cut`, `pick`, and `finalize`

Versions from tags and from the commits that start releases, `bump`, release
branches, the change requests on `main` under all three forges, through
`jp_github` (extended with pull-request create and label calls) and GitLab's
push options, picks with their moves and copies, patch releases, and tagging.
Configure JP's `bump` and `change_request_labels`, and run JP's checks on pushes
to `release/*`.
Depends on Phase 1.

### Phase 3: `due` and the reminder

Add `due` and `just release-due`, and JP's daily workflow with its reminder
issue, which the tag push closes.
Depends on Phase 2.

### Repository settings

Nothing here blocks Phase 2: `main`'s rules already accept the change requests
`cut` and `pick` open, and no rules apply to `release/*` today, so `jp release`
can push release branches.
These settings keep the record of releases from being rewritten; someone with
admin access to the repository applies them once Phase 2 has merged:

- On `release/*`, block force pushes and deletion.
- On `v*` tags, block updates and deletion.

### Phase 4: Site and jilu

Add releases to `jp vite` as a workflow: the snapshot entries, the loader, the
release notes and upgrade guide pages, the index, and the "Unreleased" page.
Install the `changelog` binary in the docs build, stop deploying the site on tag
pushes, switch the navigation to "Releases", and retire jilu.
Depends on Phase 1 and on [RFD 116]'s site phase.

### Phase 5: `review`

Depends on Phase 1.

## References

- [RFD 116: Shareable RFD, Ticket, and Site Tooling][RFD 116], the plugin family
  and the `jp vite` workflows this RFD adds to.
- [RFD 121: Changelog Fragments and Validation][RFD 121], the inputs this
  pipeline consumes and the plugin it extends.
- [RFD 001: The JP RFD Process][RFD 001], the lifecycle this RFD follows.
- [Bevy release news example] and [Bevy migration guide example], the split
  between release notes and upgrade guide, and [bevy-website
  `generate-release`], prior art for assembling them from fragments.
- [Vector releases index], prior art for the per-release page and index, and
  [Vector's release checklist], the freeze this RFD avoids.
- [Triggering a workflow from a workflow], GitHub's rules for what the workflow
  token can and cannot start.
- [GitLab commit message templates] and [GitLab push options], what GitLab
  writes into merge and squash commits and how a push opens a merge request.
- [cargo-edit], whose `cargo set-version` is JP's `bump`.

[Bevy migration guide example]: https://bevy.org/learn/migration-guides/0-18-to-0-19/
[Bevy release news example]: https://bevy.org/news/bevy-0-19/
[GitLab commit message templates]: https://docs.gitlab.com/user/project/merge_requests/commit_templates/
[GitLab push options]: https://docs.gitlab.com/topics/git/commit/#push-options-for-merge-requests
[RFD 001]: 001-jp-rfd-process.md
[RFD 099]: 099-native-macos-app-for-browsing-conversations.md
[RFD 116]: 116-shareable-rfd-ticket-and-site-tooling.md
[RFD 121]: 121-changelog-fragments-and-validation.md
[Triggering a workflow from a workflow]: https://docs.github.com/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow#triggering-a-workflow-from-a-workflow
[Vector releases index]: https://vector.dev/releases/
[Vector's release checklist]: https://github.com/vectordotdev/vector/blob/master/.github/ISSUE_TEMPLATE/minor-release.md
[bevy-website `generate-release`]: https://github.com/bevyengine/bevy-website/tree/main/generate-release
[cargo-edit]: https://github.com/killercup/cargo-edit
