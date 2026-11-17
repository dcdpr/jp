# RFD 122: Release Pipeline

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-01
- **Extends**: [RFD 116]
- **Required by**: [RFD 123]
- **Summary**: Extends `jp release` to cut, finalize, and build releases
  locally, keeping each release's notes as one document on `main`.

## Summary

This RFD extends the `jp release` plugin ([RFD 121]) with commands that cut,
tag, and build releases while keeping every release's notes on `main`.
`jp release cut` branches `release/<series>` for code and
`release-notes/v<version>` for notes, which writes the release's fragments into
one Markdown document that is the release's notes once merged into `main`; `jp
release finalize` tags the release with that document as the tag's message, and
`jp release build` runs the project's build command for it.
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
Following Bevy's model, a release's notes open with stories about its notable
features, list every other change below them, and end with an upgrade guide for
breaking changes, which the list links to so it stays short.
Once a release is cut, its notes are one Markdown document: the site serves it
as a page, the tag and the forge's release page carry it, and preparing the
release edits it like any other file, so reordering entries is moving lines.

Editing release notes can take days, and development on `main` should not wait
for it.
Cutting a release onto its own branch freezes the code in that release while
`main` continues advancing.
A fix that can't wait for the cadence ships in a release cut early, since the
cadence is a reminder, not a gate.

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

| Where                                                  | Holds                                                                                                                        |
| ------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------- |
| `main`                                                 | Development, and every release's notes: unreleased fragments at the top level of `changelog.d/`, and each release's document |
| `release/0.3`                                          | The code the 0.3.0 release ships: `main` at the cut, without `changelog.d/`                                                  |
| `release-notes/v0.3.0`                                 | A release's notes on their way to `main`: its fragments written into its document, to be merged into `main`                  |
| `v0.3.0`                                               | The tag on `release/0.3`: the release                                                                                        |
| `release/0.3.0/` in the workspace's user-local storage | A release's build: its archives, their checksums, and a manifest ([Building a release](#building-a-release))                 |

No separate state file is used:

- A release **ships** a change when its branch contains the commit that made it,
  from `main`'s history at the cut.
- A fragment is **unreleased** until a release ships its change: the commit that
  added it, or every change its `Credits` name ([RFD 121]).
  The release's notes branch then writes it into the release's document and
  deletes it, and until that merges it is still at the top level.
- A release's **document** is its notes from the cut on ([The release
  document](#the-release-document)).
- A release is **in preparation** from the commit that starts it on its branch
  until its tag, and a tag is a **finalized** release.
- A finalized release is **built** when its build directory holds a build of the
  commit its tag names.

### Commands

[RFD 121] introduces the plugin with `init`, `check`, `highlight`, and a command
for each type of change, such as `fixed`.
This RFD adds:

| Command                                            | Does                                                                |
| -------------------------------------------------- | ------------------------------------------------------------------- |
| `jp release cut [--version VERSION] [--at COMMIT]` | Starts the next minor or major release from `main`                  |
| `jp release finalize VERSION`                      | Tags a release, with its notes as the tag's message                 |
| `jp release build VERSION`                         | Builds a release's binaries with the project's build command        |
| `jp release abandon VERSION`                       | Undoes a release nobody has published, so it can be cut again       |
| `jp release render [VERSION]`                      | Prints a release's document, or the unreleased notes                |
| `jp release check --fix`                           | Gives the new entries and highlights in release documents their IDs |
| `jp release due`                                   | Succeeds when the next release should be cut                        |
| `jp release status`                                | Says where the releases stand, and what to run next                 |

`cut`, `finalize`, `build`, and `abandon` read `main`, the release branches, and
the notes branches from `origin` and from the local repository, and work in
temporary worktrees, so the checkout they run from is left alone.
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
notes_dir = "docs/releases"
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
- `notes_dir` is where release documents go, one `<version>.md` each, and
  defaults to the fragments directory.
  JP keeps them in `docs/releases`, so each document is the page its site serves
  ([On the site](#on-the-site)).
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
cannot name a minor release while an unreleased fragment has an upgrade section,
and `cut` refuses that.
The next version is computed from the highest tag or release in preparation, so
running `cut` while a release is in preparation cuts the subsequent version.
`jp release` makes no patch releases: a fix that can't wait ships in the next
minor release, cut early.

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
pinned `cargo-edit`, builds `jp` and `jp-release` from the checkout, and runs
`jp release` with the given arguments.

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
   writes every unreleased fragment and highlight into the release's document,
   `<notes_dir>/<version>.md` ([The release document](#the-release-document)),
   deletes them, and runs `bump`.
7. Prints the `git push` that publishes both branches.

<!-- end list -->

```
changelog.d/
  README.md
  .epoch
  enable-explicit.removed.md    # merged after the cut: unreleased
docs/releases/                  # JP's notes_dir
  0.2.0.md
  0.3.0.md                      # retry-idle-turns, plugin-tools, ...
```

Because both branches start from the same commit, the code on the release branch
and the entries in its document match regardless of how long the notes branch
takes to merge.
Any fragment merged to `main` after the cut point remains at the root of
`changelog.d/`, and its code is not on the release branch, so both belong to the
next release.
A correction to a fragment the cut wrote into the document, merged on `main`
before the notes branch, conflicts with the cut's deletion of that fragment: the
merge stops, and the correction is carried into the document by hand.
While a release is prepared, corrections to its notes go to its notes branch.

Whether a fragment is unreleased depends on what release branches contain its
commit, not on whether a document holds it yet.
If a fragment is still at the root of `changelog.d/` only because another
release's notes branch has not merged yet, its commit is already on a release
branch, so a subsequent `cut` leaves it for that notes branch.
`cut` never blocks on another release's notes.

Because the release branch deletes `changelog.d/`, no fragment can be written
there; release documents outside it stay as they were at the cut, and release
notes are always read from `main`.
The version-bump commit that starts the release is the only commit on a release
branch that does not originate on `main`.

### Cutting at an earlier commit

`jp release cut --at <commit>` cuts a release at an earlier commit on `main`'s
first-parent history that no existing release branch contains, rather than at
the currently checked-out commit.
The release branch starts at `<commit>`, while the notes branch starts at the
current checkout and writes only the fragments whose changes `<commit>` contains
into the document.
This lets a project backfill historical releases one period at a time by writing
fragments with explicit `Credits` naming the historical commits or pull requests
([RFD 121]); [RFD 123] uses this workflow to release JP's history.
The initial commit on the release branch removes `changelog.d/` with
`--ignore-unmatch` so commits that predate `changelog.d/` still succeed, and git
respects `GIT_AUTHOR_DATE` and `GIT_COMMITTER_DATE` when dating the release
commits and tag.

### Publishing

`cut`, `finalize`, and `abandon` publish nothing, and `build` writes only to its
build directory.
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

Preparing release notes takes as long as needed without blocking `main`.
The release's document is edited on its notes branch until it merges, and on
`main`, like any change, after:

- **Wording**: entries, highlights, and upgrade sections are edited in place.
  Two entries for one change, such as a change and its follow-up, become one by
  deleting one and adding its credits to the other's identity comment.
  Release notes describe what changed relative to the previous release: a
  feature introduced in the release is represented by a single `added` entry
  regardless of intermediate reworks, and only changes that break behavior from
  an earlier release include an upgrade section.
- **The overview** opens the document.
  The cut writes a summary to start from, naming how many entries of each type
  the release has and its highlights' titles, which preparing the release
  replaces.
  An `overview.md` written in the fragments directory before the cut becomes the
  document's overview instead ([RFD 121]).
- **The order** is the document's: highlights come first, as sections, and each
  type's entries are a list.
  Moving a section or a list item reorders them.
- **Entries and highlights added by hand** get their IDs from `jp release check
  --fix`, and an entry names its credits in its identity comment.

Code reaches a release branch only through `cut`, so every change lands on
`main` first, and a change merged after the cut ships in the next release.

### Finalizing a release

`jp release finalize 0.3.0` creates an annotated `v0.3.0` tag at the tip of
`release/0.3` in the local repository and prints the push that publishes it,
including the release branch when `origin` is missing some of its commits.
The tag message starts with the subject `Release 0.3.0` (shown by `git tag -n`),
followed by the release's document without its title, so `git show v0.3.0` shows
the whole release, upgrade guide included, without the documentation site.
The identity comments show there as text, entries' credits included.
Breaking entries link to their upgrade sections by anchor, since those sections
are in the same message; git keeps the message verbatim, including `#` headings.

`finalize` reads the document from `<notes_dir>/0.3.0.md` in the checked-out
commit, which stands in for `main` as it does for `cut`, so the notes branch
must merge first.
It refuses to run while the notes branch (local or on `origin`) has the document
and the checked-out commit lacks it, while a fragment for a change the release
ships is still at the root of `changelog.d/`, or while `jp release check` finds
a problem in the document.

Pushing the tag publishes the release's code and notes, and building its
binaries comes next ([Building a release](#building-a-release)).
The person releasing decides when: on GitHub, CI runs on every push to a release
branch and shows on its tip; elsewhere, `just ci` runs the same checks locally.

The release's notes stay on `main`: correcting them, at any time, is an edit to
its document there, which every place the notes appear shows, except the tag,
which keeps the notes the release was tagged with.

### Building a release

`jp release build 0.3.0` builds a release's artifacts on the local machine:

1. Checks out the commit named by `v0.3.0` (or the tip of `release/0.3` while
   the release is in preparation) in a temporary worktree.
2. Runs the configured `build` command from the current workspace root (so
   historical releases build with the current build script), substituting
   `{version}`, `{source}` (the worktree path), `{out}` (an empty staging
   directory), and `{profile}` (`release`, or `quick` with `--quick`).
   Build output streams to the terminal unless `--quiet` is passed.
3. Validates the contents of `{out}`: the artifact files, `checksums.txt`
   (containing a SHA-256 entry for each artifact in `sha256sum` format), and
   `manifest.json` (recording the version, commit, profile, and target triple
   for each artifact), with no extra files allowed.
   If the command fails or validation finds any problems, the staging directory
   is discarded and all problems are reported together.
4. Moves the validated directory to `release/0.3.0/` under workspace user-local
   storage (`jp path user-workspace`), replacing any previous build for that
   version.

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

### Abandoning a release

`jp release abandon 0.3.0` retracts an unpublished release (one whose tag has
not been pushed to `origin`) so it can be cut again:

- Refuses if a newer release exists (releases must be abandoned from newest to
  oldest) or if `origin` already has tag `v0.3.0`.

- Deletes the local tag `v0.3.0`, notes branch `release-notes/v0.3.0`, and
  release branch `release/0.3`.

- Creates `release-notes/abandon-v0.3.0` (from the notes branch if unmerged, or
  from the current checkout if already merged), splitting the release's document
  back into fragments at the top level of `changelog.d/`, with its wording edits
  kept:

  - Each entry becomes a fragment named after its ID, with its type from its
    section, its ID and credits as metadata ([RFD 121]), and its upgrade
    section.
  - Each highlight becomes a highlight note named after its ID, with its authors
    and credits from its byline.
  - The overview becomes `changelog.d/overview.md`, which the next cut uses.

  The order is lost: the next cut lists the restored fragments by their first
  credit.
  `abandon` refuses a document `jp release check` refuses, since it could not
  split it reliably.

- Prints the `git push` commands to delete the corresponding branches on
  `origin`.

### The release document

Each release's notes are one Markdown document, `<version>.md` in the notes
directory ([Configuration](#configuration)).
`cut` writes it from the release's fragments and highlights, and from then on it
is edited like any other file: the site shows it as the release's page,
`finalize` puts it in the tag, and a publisher puts it on the release's page on
a forge ([RFD 123]).

An example document, as `jp release cut` wrote it for 0.3.0 and preparing the
release edited it (entries are paraphrased from recent pull requests, with an
invented breaking change and final entry):

````markdown
# Release 0.3.0

JP 0.3.0 lets the assistant call one tool on many inputs at once, and lets the
plugins you install give it new tools.
It also changes how JP finds plugins, so read the upgrade guide before you
upgrade.

## Tool calls fan out

<!-- C-1n2q400 -->

By @JeanMertz in [#1182](https://github.com/dcdpr/jp/pull/1182) and
[#1183](https://github.com/dcdpr/jp/pull/1183).

When the assistant needs the same tool on several inputs, such as reading ten
files, it asks once and JP runs the calls side by side.
Each result still shows up under its own call, and one failing call does not
stop the others.

## Changelog

### New features

- <!-- C-1n2q401 #1182 -->
  The assistant can call one tool on many inputs in a single request.
- <!-- C-1n2q402 #1243 -->
  `jp conversation show` shows the conversation's model.

### Changed

- <!-- C-1n2q403 #1216 -->
  JP finds a command plugin by the manifest its binary embeds, not by its file
  name.
  **Breaking**: see
  [the upgrade guide](#command-plugins-are-found-by-their-manifest).

### Bug fixes

- <!-- C-1n2q404 #1207 -->
  A turn that stalls while idle is retried instead of failing.
- <!-- C-1n2q405 4e1f0a2b3c4d -->
  The help for `jp query --tmp` names the right default.

## Upgrade guide

### Command plugins are found by their manifest

<!-- C-1n2q403 -->

Before 0.3.0, `jp <name>` ran a binary called `jp-<name>` from your `PATH`.
JP now reads the manifest each plugin binary embeds, which names the commands it
serves, and a plugin built before 0.3.0 has none, so JP no longer finds it.

Rebuild the plugin against 0.3.0, or reinstall it:

```sh
jp plugin install <name>
```
````

A document has this shape, which `jp release check` holds it to:

- It opens with `# Release <version>`, followed by the overview.
- Each highlight is a `##` section before `## Changelog`: its identity comment,
  its byline, `By <authors> in <credits>.`, and then its story.
- `## Changelog` holds a `###` section per type, under the type's heading
  ([Configuration](#configuration)), each listing that type's entries.
  An entry is a list item that opens with its identity comment, alone on the
  marker line.
- When an entry breaks something, `## Upgrade guide` closes the document with a
  `###` section per such entry, which opens with the entry's identity comment.
  The entry ends with `**Breaking**: see [the upgrade guide](#<anchor>).`,
  linking to its section.

An **identity comment** is an HTML comment naming an entry or a highlight by a
`C-` ID, which `cut` allocates from the fragments directory's epoch ([RFD 121]);
an entry's comment also names its credits, as `Credits` fields write them
([Credits and links](#credits-and-links)).
The site and forges render a comment as nothing, so the comments carry the data
`abandon` and `render --format json` read without showing it, while `git show`
shows them as text.
A comment written on the same line as an entry's text would turn that line into
raw HTML, so it stays alone on the marker line, where `comfort` keeps it, and
`check` refuses one that is not.
A highlight's authors and credits are its byline, which shows on the page and
which `check` requires to parse.

`check` holds the document of a release in preparation to the whole shape,
including its type headings, and requires every entry to name a credit.
A tagged release's document is held only to unique, well-formed identity
comments, since it is frozen and the type headings in configuration may change
later.
`jp release check --fix` gives an entry or highlight added by hand its ID, and
writes each breaking entry's link from its upgrade section's heading, so
renaming the heading does not break the link.

### The release model

The `changelog` crate reads a release model from a release's document, or, for
everything no finalized release ships, builds one from the fragments at the root
of `changelog.d/` and the documents of releases without a tag, so a release's
entries count as unreleased until it is tagged, however early its notes branch
merged.
Consumers that need the parts of the notes, rather than the document, read the
JSON output:

```json
{
  "format": 2,
  "version": "0.3.0",
  "date": "2026-10-28",
  "overview": "JP 0.3.0 lets the assistant call one tool on many inputs ...",
  "highlights": [
    {
      "id": "C-1n2q400",
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
      "id": "C-1n2q403",
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
  A breaking entry's `body` leaves out its link to the upgrade guide.
- `id` is the entry's or highlight's ID, and `null` for a fragment no release
  has cut yet.
- `type` is the raw fragment type ID from [RFD 121], never a display heading;
  consumers map type IDs to headings.
- Each credit has a `kind` (`change_request` or `commit`) and a forge-formatted
  `label` (`#1182` on GitHub, `!1182` on GitLab, or a short commit hash), so
  consumers can render `[label](url)` without forge-specific logic.
  When `forge = "none"`, credits are commits with no `url`.
- `date` is `null` for a release in preparation, and both `version` and `date`
  are `null` for the unreleased model.
- A document's highlights and entries are listed in its order, and the
  unreleased fragments by their first credit and then by file name, for
  deterministic output.
- `format` versions the JSON schema so consumers can reject unsupported
  versions.

Both `changelog render` (the standalone crate binary) and `jp release render`
accept `--format json` or `--format markdown`.
With a version, the Markdown is the release's document as it is.
Without one, the unreleased fragments and highlights, and those of documents
without a tag, are rendered into the same shape under `# Unreleased`, with
entries' credits in identity comments that hold no ID, since the cut allocates
IDs.
The `changelog` binary does not read JP configuration; it receives the fragments
directory, notes directory, forge, project web address (`repository`), and
section headings via CLI flags, which `jp release render` populates from the
plugin configuration.

Every consumer reads from `main`, where the release documents live:

- `jp release render` prints them, to read while a release is prepared or to
  hand to another tool.
- `jp vite` serves them as pages on the site ([On the site](#on-the-site)).
- A publisher, such as [RFD 123]'s `jp release github`, puts a release's
  document where it publishes the release, and again when the document changes
  on `main`.

### Credits and links

Credits come from `main`'s history, read when a release is cut.
For each fragment and highlight, the crate reads the commits that wrote or
changed it while it sat at the top level of `changelog.d/`, following renames.
A commit's change request is read from its message or, when the commit reached
`main` through a merge, from the merge commit's message.
A fragment that lists `Credits` takes those instead, commits included.
The cut writes an entry's credits into its identity comment and a highlight's
into its byline, where they are text from then on: a correction credits nobody,
and editing the comment or the byline changes the credits.
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

Because credit resolution requires Git history, `cut` and rendering the
unreleased notes reject shallow clones rather than emitting incomplete credits;
a release's document needs no history.

An entry's credits stay in its identity comment, so the pages that render the
notes, read by people deciding whether to upgrade, show none, while the source
and the tag message, read by the people working on the project, show them.
A highlight's byline (`By @author in #1182.`) is part of its story, and always
names its credits.

### Release workflow

`jp release due` exits zero when `cadence` has elapsed since the latest minor or
major release was cut, that release has been finalized, and `main` has at least
one unreleased fragment (without one, `cut` would have nothing to release).

1. **Cut.** A maintainer runs `jp release cut`, when `due` says so or on any
   other day, and pushes the two branches it made.
2. **Prepare.** They and others edit the release's notes on its notes branch,
   for as long as it takes, then merge the notes branch into `main`.
3. **Finalize.** They run `jp release finalize` from `main` once the notes
   branch has merged and the release branch's checks pass, and push the tag.
4. **Build.** They run `jp release build`, and publish what it built wherever
   the project publishes releases ([RFD 123]).

`jp release status` reports the next step, so maintainers do not have to
remember the order.
It reads the same state as the other commands without changing anything, and
reports, in this order:

- whether the checkout is missing commits from `origin/main`, which `cut` and
  `finalize` refuse;
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
`jp vite sync` writes `fragments_dir`, `notes_dir`, `forge`, `repository`, and
`headings` into its settings snapshot with the other plugins' settings, and
refuses a `notes_dir` outside the site: a release's page is its document, so the
document has to be a file the site serves.
JP's documents are in `docs/releases/`, served under `/releases/`, and the
navigation's "Change Log" entry is replaced with "Releases".

- **A release's page** is its document, rendered as any page is, so "View
  Markdown" serves the document, and no page shows an entry's credits.
- **The Unreleased page** is generated: the site writes
  `<notes_dir>/unreleased.md` from `changelog render` when it starts, and `jp
  vite serve` writes it again when a fragment or a document without a tag
  changes.
  Git ignores the file.
- **A release in preparation**, whose document has no tag yet, is left out of
  the built site, which `just preview-docs` serves, and its entries are on the
  Unreleased page.
  Under `jp vite serve`, its document is also shown as a draft, with a notice
  above its content, so its notes can be read as they will appear.
- **The index** lists the documents with the dates of their tags, and the theme
  shows a release's date above its page.

`jp release render 0.3.0` prints a document without the site.

Under `jp vite serve`, and under `jp vite build`, which builds the site as
`vitepress build` does, the site renders the Unreleased page and reads release
dates with the `jp-vite` binary itself, run as `jp-vite changelog`, which `jp
vite` names to it in `JP_VITE_BIN`.
A workspace then needs nothing but `jp` and its plugins to build its site.
`jp vite build` brings the settings snapshot up to date first, as `serve` does,
and refuses while a file `jp vite sync` writes is out of date.
The `package.json` `jp vite init` writes runs both, as its `dev` and `build`
scripts.
A `vitepress build` started by hand runs the `changelog` binary on `PATH`.

JP's docs build runs `vitepress build` directly, so it needs no `jp`; it
compiles and installs the `changelog` crate binary in place of `jilu`, without
compiling the rest of the workspace.
The docs workflow already fetches full Git history, which credit resolution
requires.

Release pages are always built from `main`.
A release tag holds the documents of earlier releases as they were at its cut,
but not its own, which merges on `main` later, and no fragments, so building the
site from a release tag would show stale notes and must not overwrite the
deployment from `main`.
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

The `/releases/` pages replace the committed `docs/change-log.md` file.
The `build-changelog` recipe in the `justfile`, the `jilu` dependency, and the
docs-build step that ran `jilu` are removed.

## Drawbacks

- **The project owns the assembly logic.** `jilu` is replaced by code JP
  maintains.
  The logic is small (parse, credit, model, render), but it is now ours to test
  and fix.
- **A cut lands in two steps.** The release branch is pushed at once; `main`
  follows when the notes branch merges, and until then the release's fragments
  are still at the top level.
- **Publishing is a step to forget.** The commands push nothing, so a cut nobody
  pushed blocks the next cut until it is pushed or deleted.
- **A local build is as trustworthy as the machine it ran on.** It carries
  checksums, which show a file is the one published but not how it was built,
  until a remote builder that attests its builds replaces it.
- **No patch releases.** A fix that can't wait ships in a release cut early,
  with everything else merged since.
- **Branches and documents accumulate**: a release branch and a document per
  release, 26 of each a year at JP's cadence.
- **Tags off `main`.** Every release is tagged on its branch, past the commit
  that deletes `changelog.d/`, so `git describe` on `main` finds no release.
  A release's starting point on `main` is the parent of its branch's first
  commit.
- **Workflows that run `jp release` compile `jp` and `jp-release`** until
  releases publish both; the daily `due` is the main one.
- **Credits depend on merge messages.** A forge or project that writes no change
  request into its merge or squash commits gets commit credits instead.
- **Corrections to cut fragments conflict.** Between a cut and its notes branch
  merging, an edit on `main` to a fragment or highlight the cut wrote into the
  document conflicts with the cut's deletion of it, and is carried into the
  document by hand.
- **Documents keep their wording.** Links and headings stay as each release's
  cut and edits left them, so moving to another forge or renaming a type's
  heading means editing every document, as for any other page on the site.
- **`abandon` loses the order.** The next cut lists restored fragments by their
  first credit.

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
  fewer, but the version bump would land on `main` at the cut, and patch
  releases would need release branches introduced then.
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
- **Workflows that prepare releases or open pull requests.** A workflow's pushes
  and pull requests wait for approval unless it holds a GitHub App token, and
  either way the flow needs GitHub.
- **A check on `main` that fails when a release is overdue.** Every pull request
  would show it red, and no change in it can fix it, which teaches people to
  ignore red.
- **A directory of fragments per release, rendered when the site builds**, as
  this RFD first specified.
  Each entry stayed a file with credits from history, but no page was a file:
  "View Markdown" had nothing to serve, the order needed a separate
  `.order.json`, editing notes from the site needed code specific to releases,
  and every build rendered every release again from git history.
- **Entries as records, like tickets**, each a file with its ID, version, and
  type as metadata, listed on release pages by a component.
  Each entry keeps its own file and history, but the page is still not a file,
  and reordering or editing entries on the site needs an interface for release
  notes, where a document needs only Markdown editing.
- **Generating release pages as files when the site builds.** "View Markdown"
  would work, but the files would be output rather than source: an edit to one
  is lost at the next build.
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
- **Patch releases.** A release ships what `main` held at its cut.
  Picking fixes onto a release branch and releasing them as a patch release can
  come back once a release line needs maintaining.
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
- **A breaking change that has to ship in a release that cannot break.** From
  `1.0` on, `cut --version` refuses a minor release while an unreleased fragment
  has an upgrade section.
  Shipping it means `jp release abandon` on the release in preparation and
  cutting a major one.
- **Lockfile conflicts.** The notes branch changes `Cargo.lock` through `bump`,
  and stays open while the notes are prepared, so a change to `Cargo.lock`
  merged meanwhile may conflict with it; running `bump` again after merging
  `main` into it resolves it.
- **Highlights extended across a cut.** Highlights are edited by every pull
  request that builds their feature ([RFD 121]), so a pull request merged after
  the cut can extend a highlight the cut wrote into the document.
  The extension then conflicts with the cut's deletion of the highlight, so
  whoever resolves it decides whether it belongs in this release's story or in a
  highlight for the next release; it describes work this release does not ship,
  so usually the next.
- **Hand edits that break the shape.** An entry pasted without its identity
  comment, or a comment typed on the line of the entry's text, breaks the
  document's shape.
  `check` refuses both in CI, and `check --fix` gives a new entry its ID.

## Implementation Plan

### Phase 1: Model, credits, and rendering

Implement the release model, forge credit resolution (`github`, `gitlab`, and
`none`), and Markdown/JSON rendering in `crates/internal/changelog` (`changelog
render`), and expose `jp release render` with the `forge` and `headings`
options.
Depends on [RFD 121] Phases 2 and 3.

### Phase 2: `cut` and `finalize`

Implement version computation from tags and release branches, the `bump` hook,
release branch and notes branch creation in the local repository from the
checked-out commit (printing the `git push` that publishes them), and `finalize`
(with the release notes as the tag message).
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

Implement `cut --at` (filing fragments by the commits referenced in their
`Credits` fields and supporting trees without `changelog.d/`) and `abandon`.
Have `cut` instruct the user to merge the notes branch into `main`.
Depends on Phase 2.

### Phase 6: Building

Implement `jp release build` with the `build` option and `--quick`, output
directory validation (`manifest.json` and `checksums.txt`), and `status`
reporting finalized releases without a publishable build.
`--remote` and `builders` are deferred until a remote builder is added.
Depends on Phase 2.

### Phase 7: Links and drafts

Add the `repository` option across release-note rendering and `jp vite`'s
settings snapshot, and render draft pages for untagged releases in preparation
under `jp vite serve`.
Depends on Phase 4.

### Phase 8: Sites built with `jp vite`

Add `jp vite build` and have the `jp-vite` binary render release pages directly
(via `jp-vite changelog`) under both `serve` and `build`, so only standalone
`vitepress build` invocations require `changelog` on `PATH`.
Update the `package.json` template written by `jp vite init` to run `jp vite
serve` and `jp vite build` in its `dev` and `build` scripts.
Depends on Phase 4.

### Phase 9: One document per release

Write each release's notes as one document at the cut ([The release
document](#the-release-document)):

- the `notes_dir` option, `C-` IDs from the fragments directory's epoch, and
  identity comments carrying credits;
- `cut` writing the document and deleting the fragments it wrote, and `abandon`
  splitting it back;
- `finalize`, `render`, `status`, and `check` reading documents, `check --fix`,
  and the JSON model's format 2;
- removing release directories, `.order.json`, and `render`'s `--page`,
  `--upgrade-url`, and `--no-credits`;
- on the site, serving documents as pages, generating the Unreleased page, and
  leaving documents without a tag out of the build.

The fragment format changes in [RFD 121] and the GitHub publisher's in [RFD 123]
land with this phase.
Depends on Phases 1 to 8.

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
