# RFD 121: Changelog Fragments and Validation

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-01
- **Extends**: [RFD 116]
- **Required by**: [RFD 122]
- **Summary**: Contributors record user-facing changes as typed fragments and
  highlight notes, written by a `jp release` plugin and required by CI.

## Summary

This RFD defines two user-facing input formats, written in the pull request that
makes a change:

- **Changelog fragments**: one short entry per user-visible change, grouped by
  impact type.
- **Highlight notes**: editorial, feature-level notes for the changes worth a
  story.

Both are Markdown files with YAML frontmatter in a fragments directory,
`changelog.d/` by default.
A `jp release` command plugin writes and validates them, at the terminal and as
a tool the assistant calls.
In JP's CI, a required check keeps a pull request red until it has a fragment or
the GitHub label `no-changelog`.
Assembling and publishing releases from these inputs is [RFD 122].

## Motivation

JP is approaching its first official release and has no structured way to record
what changed for users.
The changelog is generated from git history by `jilu`, which reads
conventional-commit subjects.
That is the wrong source for user-facing notes.

Commit messages serve developers reading the history of the code.
Release notes serve a user who wants to know what a feature does and how it
affects them.
These are different audiences: `fix(cli, md): Preserve trailing separator after
terminal tight list (#756)` is a precise commit subject and a useless release
note.
Deriving notes from commit subjects forces one sentence to serve both, and it
serves neither well.

Recording the note in the pull request, while the context is fresh, is the fix.
Per-file fragments also sidestep the merge conflicts a single shared changelog
file produces when many branches edit it at once.

Doing this right matters more for the first release than for any later one: it
draws the most attention, and it sets the template.
So the format supports two tiers from the start, a terse exhaustive entry and a
rich editorial highlight, rather than only the terse form.

None of this is specific to JP.
The plugins of [RFD 116] serve tickets, RFDs, and the site to any JP workspace,
and the same projects benefit from recording what changed for their users.
So the formats ship as a plugin in that family, and the code that reads them as
a library that reads no configuration, which other tools can use.

## Design

### Two input formats

Both live in the fragments directory and carry YAML frontmatter:

- A **changelog fragment** is one entry per user-visible change.
  It is terse, typed, and exhaustive: every user-facing change gets one, so
  nothing is lost.
- A **highlight note** is an editorial section for a notable feature.
  It is prose, titled, and selective: only changes worth a story get one, and a
  single note can span several pull requests.

A release ([RFD 122]) gets release notes, with an introduction, the highlight
notes, and every fragment grouped by type, and, when it breaks something, an
upgrade guide.
This RFD defines the two formats, the code that writes and validates them, and
JP's CI, not the rendering.

### Changelog fragment

A file directly in the fragments directory, named with a descriptive slug:

```markdown
---
type: added
---
`tools.<name>.enable` splits into `state` and `allow_toggle`, so you can pin a
tool on or off while still letting the assistant request the other state.
```

`type` is one of seven ids, classifying **user impact**: the Keep a Changelog
categories plus `performance`.

| `type`        | Used for                                        |
| ------------- | ----------------------------------------------- |
| `added`       | A new capability the user can now reach.        |
| `changed`     | A change to existing behavior.                  |
| `deprecated`  | Something still working but slated for removal. |
| `removed`     | Something taken away.                           |
| `fixed`       | A bug fix the user would notice.                |
| `security`    | A fix with security relevance.                  |
| `performance` | A user-visible performance improvement.         |

The ids are fixed.
A fragment means the same in every workspace, a contributor moving between
projects writes the same types, and validation needs no configuration.
Release notes show the types in the table's order, each under a heading a
workspace can rename ([RFD 122]).

The type is not a conventional-commit type.
Conventional-commit types (`feat`, `refactor`, `test`, ...) classify the nature
of a code change for developers; the fragment classifies its effect on users.
The two disagree often enough to matter: an internal `feat(tools)` changes
nothing a user sees, and a `refactor(cli)` can change what they see.
Changes with no user impact, which covers most `build`, `ci`, `docs`,
`refactor`, and `test` work, produce no fragment.

Conventional commits are not required, but a project that writes them gets more
from JP's CI: it uses the type in a pull request's title as a hint for the
`no-changelog` label, and a `!` in the title to require an upgrade section (see
[CI validation](#ci-validation)).
JP keeps writing them.

The filename is a descriptive slug (`enable-decomposition.md`), unique within
its directory; it only avoids collisions.

#### Breaking changes

A fragment for a change that requires users to act carries its section of the
release's upgrade guide: a paragraph that starts with `BREAKING CHANGE:`, and
the paragraphs after it.

````markdown
---
type: removed
---
`tools.<name>.enable` no longer accepts the legacy string `"explicit"`.

BREAKING CHANGE: `enable = "explicit"` is no longer accepted

`"explicit"` was the last of the string forms `enable` accepted before 0.2.0.
Replace it with the table form, which means the same thing:

```toml
# 0.2
enable = "explicit"

# 0.3
enable = { state = false, allow_toggle = "if_named" }
```
````

Such a fragment has three parts:

- **The entry**, everything before the marker, which the release notes list in
  its type's group, marked as breaking and linked to its upgrade section.
- **The title**, the rest of the marker's paragraph, which heads the fragment's
  upgrade section in the upgrade guide ([RFD 122]).
  It names what broke, the way a reader would search for it.
- **The body**, every paragraph after the title, code blocks included: what
  changed, why, and how to upgrade, with before-and-after examples where they
  help.

`BREAKING-CHANGE:`, the Conventional Commits synonym, works as the marker too.
All three parts must be non-empty, and a fragment has at most one upgrade
section.

The upgrade section lives in the body rather than the frontmatter so it is
ordinary Markdown: it renders, wraps, and goes through the same formatter as the
rest of the file.
The token is borrowed from Conventional Commits because it is familiar; a
fragment does not need a conventional commit to use it.
A highlight note cannot carry an upgrade section; the fragment for the change
does.

#### Credits

Fragments do not name the change requests behind them.
A *change request* is a forge's unit of review: a pull request on GitHub, a
merge request on GitLab.
The author does not know its number when writing the fragment, and having CI
write it back would need a bot identity and commits to contributor branches the
project does not want.

The release pipeline ([RFD 122]) credits each fragment from `main`'s history
instead: every change request whose commits wrote or changed it while it sat at
the top level of the fragments directory.
Edits after a release took it correct its wording and credit nobody.
Where the history names no change request, the entry credits the commit.
An optional `change_requests` list in the frontmatter replaces the derived
credits for the rare case where they are wrong; its numbers are change requests
on the workspace's forge.

Each user-visible change adds its own fragment, follow-ups included: a pull
request that extends an earlier change describes what it adds in a new fragment,
whether or not the earlier one has been released.
An existing fragment is edited only to correct its wording.
The rule makes a fragment's text describe the code that added it, which matters
because a fragment can move into a release while a pull request that edits it is
open ([Released fragments](#released-fragments)): git carries the edit to the
moved file when the pull request merges, which is right for a correction and
would be wrong for a description of new work.
A feature built across several pull requests adds a fragment for each
user-visible change they make, and each of them edits the shared highlight,
which credits all of them.

### Highlight note

A file in `highlights/` under the fragments directory, for a feature worth an
editorial section in the release notes:

```markdown
---
title: "Unified editor and inline reply widget"
authors: ["@JeanMertz"]
---
Every prompt in JP now runs through a single editor service, and short replies
can be written inline without leaving the conversation view. Longer edits still
open your editor, so the two paths share one backend and one set of keybindings.
```

- `title`: the section heading in the release notes.
- `authors`: contributors credited for the feature.
- `change_requests`: optional.
  A highlight is credited the way a fragment is, to every change request that
  wrote or changed it, so a feature built across several pull requests is one
  note, edited as later ones land, and each of them is credited.
  A list here replaces the derived credits, for a feature with change requests
  that never touched the note.

Highlight notes are selective and editorial.
Most changes get only a changelog fragment; a highlight is for the handful of
features that deserve a narrative.
A change with a highlight still gets a fragment, so it also appears in the
exhaustive list.
Unlike fragments, highlight notes carry no `type` and no upgrade section.

A highlight describes what is on `main`.
Releases are cut from `main` on a schedule, so any commit can end up in one, and
work merged in pieces needs a highlight that is true after each piece.
A feature that is still hidden gets its highlight in the pull request that makes
it visible.
One that is visible but incomplete gets a highlight describing what is there,
which later pull requests extend.

### Released fragments

Cutting a release ([RFD 122]) moves the unreleased fragments and highlights from
the top level into a directory named after its version, through a pull request
into `main`, and picking a later change into a release moves that change's
fragment the same way:

```
changelog.d/
  README.md
  enable-explicit.md          # unreleased
  highlights/fan-out.md       # unreleased
  0.3.0/
    overview.md
    retry-idle-turns.md
    highlights/plugin-tools.md
```

A fragment in a version's directory belongs to that release; one at the top
level is not yet filed into a release, and [RFD 122] decides which release ships
it.
Released fragments stay on `main` for good.
`main` is the only branch on which fragments are written, moved, or corrected;
release branches carry code and have no `changelog.d/` ([RFD 122]).
The formats are the same in both places, and validation covers released
fragments too, so a correction to an old release's notes is checked like any
other change.
A version's directory may also hold `overview.md`, a release's introduction,
which belongs to [RFD 122].

### Crates and the `jp release` plugin

The code lives in two crates, split the way [RFD 116] splits the ticket format
from `jp ticket`:

- **`crates/internal/changelog`** is the library: it parses and validates both
  formats, and [RFD 122] adds crediting and rendering to it.
  It reads no configuration and does not depend on JP's plugin protocol.
  Whatever differs between projects arrives as an argument, so another release
  or changelog tool can use it, and configuration stays the plugin's job.
  Its small `changelog` binary validates a fragments directory, for CI.
- **`crates/plugins/command/release`** is the `jp release` command plugin: a
  library with a thin `main`, built as `jp-release`, configured through
  `plugins.command.release.options`, and installed in the same `cargo install`
  as [RFD 116]'s plugins so they share a revision.
  It holds what needs the host: the commands, their configuration, the composer,
  and the tools.

This RFD's settings:

```toml
[plugins.command.release]
run = "allow"

[plugins.command.release.options]
fragments_dir = "changelog.d"
```

`fragments_dir` is optional and defaults to `changelog.d`, relative to the
workspace root.
`run = "allow"` admits the plugin without asking ([RFD 077]), which a plugin
rebuilt from the checkout needs.

| Command                        | Does                                                                                      |
| ------------------------------ | ----------------------------------------------------------------------------------------- |
| `jp release init`              | Creates the fragments directory, `highlights/`, and a `README.md` describing both formats |
| `jp release add [--highlight]` | Writes a fragment, or a highlight, asking for missing fields through the host's composer  |
| `jp release check`             | Validates every file in the fragments directory (see [CI validation](#ci-validation))     |

`add` offers the seven types as a picker, with each one's description from the
table as its hint, takes the body from the composer, and derives the slug from
the body unless `--slug` names one.
It writes through the validation `check` runs, so an entry it writes always
passes.

`add` is also served as a plugin tool, configured in JP the way `ticket_create`
is:

```toml
[conversation.tools.changelog_add]
source = "plugin.command.release.add"
style.parameters = "tool"
```

The tool writes fragments, not highlights.
A highlight is a few paragraphs of editorial prose that a person shapes, and the
assistant edits it as a file like any other document.
`style.parameters = "tool"` has the plugin preview the fragment before writing
it.
The tool's `type` parameter is the fixed list of ids, so its definition mirrors
nothing from configuration: `jp release tools init` writes it once, there is no
`tools sync`, and as in [RFD 116] the command goes once a plugin can describe
the tools it serves.
JP enables the tool in the personas that implement changes (`dev`,
`rfd-implementor`), and the `pr-reviewer` persona checks that each fragment
describes what changed for the user.

### CI validation

JP's CI enforces three rules:

1. **Format**: every file parses against the schema below, and unknown
   frontmatter fields are rejected.

   Fragment frontmatter:

   ```yaml
   type: added | changed | deprecated | removed | fixed | security | performance
   change_requests: list of positive integers # optional, replaces the derived credits
   ```

   The fragment body must be non-empty after trimming surrounding whitespace,
   and an upgrade section must follow the rules in [Breaking
   changes](#breaking-changes).

   Highlight frontmatter:

   ```yaml
   title: non-empty string
   authors: non-empty list of non-empty strings
   change_requests: list of positive integers # optional, replaces the derived credits
   ```

   Unknown fields are rejected rather than ignored.
   These are contributor-authored release inputs, and strict parsing turns a
   typo like `change_request:` into a CI error instead of a silently dropped
   field.

2. **Presence**: a pull request into `main` passes when it adds a fragment at
   the top level of the fragments directory, or carries the GitHub label
   `no-changelog`.
   Editing a fragment does not count: edits correct wording
   ([Credits](#credits)), so a pull request that only corrects a fragment
   carries the label.
   A highlight alone does not count: the grouped changelog is built from
   fragments, so a highlight-only pull request would produce a release story
   with no entry in the list.
   A released fragment does not count either: correcting an old release's notes
   changes nothing in the software, so such a pull request carries the label.
   The label is the only input the rule reads; whether a change is user-facing
   is the judgment that decides whether the label applies (see the auto-apply
   workflow below), not something the rule computes.

3. **Breaking change**: a pull request into `main` whose title marks a breaking
   change with `!` (`feat(cli)!: ...`) fails unless a fragment it adds has an
   upgrade section, or it carries `no-changelog`, for a change that breaks
   something users never see.
   A project that does not write conventional commits never triggers this rule.

The set of paths allowed in the fragments directory is closed.
A fragment is exactly `<slug>.md` and a highlight exactly
`highlights/<slug>.md`, either at the top level or in a released version's
directory, which is named after a semantic version and also holds `overview.md`.
`README.md` at the top level is ignored.
Everything else is a CI error rather than a silently ignored file, so it fails
loudly instead of vanishing at assembly: another extension (`.txt`,
`.markdown`), a misspelled `overview.md`, a directory that is not a version, a
nested directory, or a stray file under `highlights/`.
`README.md` documents both formats; `highlights/` carries no README of its own.

The rules are wired as two checks, because their triggering needs are opposite.

**Format validation** is the `just changelog-ci` recipe, a task in the
`rust.yml` matrix that runs the `changelog` binary.
The type ids are fixed, so validation needs no configuration and no `jp`: the
task builds only the `changelog` crate.
The binary takes the fragments directory as an argument, defaulting to
`changelog.d`; `jp release check` calls the same function with the directory
from configuration.
The task is filter-scoped like the others, to `changelog.d/**` and the crate (so
a parser change revalidates the corpus), and also runs on pushes to `main`.

**The changelog gate** runs presence and breaking change.
JP's branch protection lists it as a required status check, so a red gate blocks
the merge.
It runs on every pull request with no path filter: a required check that does
not run never reports, and a check that never reports blocks the merge forever.
It cannot live in the `rust.yml` matrix for the same reason, since the matrix
skips tasks whose paths did not change, and because it has to rerun when a label
or the title changes: GitHub cannot filter `labeled`, `unlabeled`, and `edited`
events by what changed, and adding them to `rust.yml` would rerun (and, off
`main`, cancel) the entire matrix on every label event.
So the gate is its own small workflow, triggered on `opened`, `synchronize`,
`reopened`, `labeled`, `unlabeled`, and `edited`, with its own concurrency
group.
It needs no Rust toolchain: the labels and the title come from the pull-request
event payload, "does this pull request add a top-level fragment" is a `git diff
--diff-filter=A` against the base, and an upgrade section is a `grep` for its
marker.
Those events rerun only this workflow, in seconds.

The gate tells a contributor what is wrong in two places, since GitHub shows a
check as no more than a name and a status: its job name, `Changelog fragment or
no-changelog label`, which reads as the requirement in the list of checks, and
an error annotation on the check's page naming the fix, such as "Add a fragment
with `jp release add`, or add the `no-changelog` label."

The gate is scoped to pull requests: it exists only where the label does.
Pushes to `main` and local invocations run format validation only.
The pull requests `jp release` opens to cut a release or pick into one ([RFD
122]) move fragments rather than add them, and pass the gate by the
`no-changelog` label it gives them.
Release branches receive commits only from `jp release`, never pull requests,
and have no fragments, so neither check runs there.
Fork pull requests are read-only here: the gate reads the label from the event
payload and needs no label mutation to function (the auto-apply workflow below
is a separate, fork-limited convenience).

A separate, best-effort workflow keeps the label in sync as a default a human
can override:

- On a pull request's opening it applies `no-changelog` when the conventional
  type in the title is non-user-facing (`build`, `ci`, `docs`, `refactor`,
  `test`), or when the diff touches only non-shipping paths (docs, CI, internal
  tooling).
  It never auto-applies for the user-facing types (`feat`, `enhance`, `fix`,
  `perf`).
  A title without a conventional type, or with one outside both lists (`chore`
  is in active use despite being absent from the documented taxonomy), falls
  through to path classification alone.
- It never fights a manual change: once a human adds or removes the label, that
  choice is final and the workflow does not undo it.
  The automation is a default, not an authority.

The gate reads the label, not the title, on purpose: the conventional type is a
lossy predictor of user impact.
An internal `feat(tools)` is not user-facing; a `refactor(cli)` might change
observable behavior.
The label absorbs that judgment and stays human-overridable; the auto-apply is a
convenience addable later without changing the gate.
(Auto-apply needs `pull-requests: write`, unavailable to fork pull requests
without `pull_request_target`; on a fork pull request a maintainer applies the
label.)

The gate, the label, and the auto-apply are JP's GitHub integration, not part of
the plugin.
A workspace without CI, or on another forge, uses `jp release` all the same and
simply has no gate.

## Drawbacks

- **A new artifact in every user-facing pull request.** Contributors write an
  entry and reviewers check it reads well.
  This is the intended cost, but it is friction the project did not have before.
- **A decision on every pull request.** The gate is required, so even a one-line
  chore needs the label before it merges, until the auto-apply workflow sets it.
- **Two formats to learn.** Mitigated: most pull requests only ever write the
  terse fragment; highlights are rare and reserved for notable features.
- **Auto-apply label management is GitHub-specific and fork-limited.** It is
  best-effort polish; the gate works without it.

## Alternatives

- **Keep `jilu` / git history as the source.** Rejected: it conflates the
  developer and user audiences, which is the whole motivation.
- **A single `CHANGELOG.md` edited per pull request.** Rejected: it produces a
  merge conflict on nearly every concurrent branch.
  Per-file entries are the standard fix.
- **Store the pull-request number in frontmatter, backfilled by CI.** Rejected:
  requires a bot identity and CI commits to contributor branches.
  Deriving from git removes the problem.
- **towncrier (Python).** Rejected: it reintroduces a non-Rust toolchain
  dependency the project can own in a few hundred lines of Rust.
- **One tier only (terse fragments, no highlights).** Rejected: the first
  release deserves editorial prose, and adding a highlight format later means
  the first and most-read release misses it.
- **The commit type as the fragment type.** Every pull request already carries
  one.
  But it classifies the code change, not its effect on users, and a pull request
  has one type where it can make several user-visible changes.
  [VRL]'s pull-request titles follow conventional commits (its release tool
  landed as `chore(releasing): ...`), and its changelog ignores them: a
  fragment's type is part of its filename.
- **A changelog generated from pull-request titles, with highlights written on
  top.** This is [Bevy's process][bevy-release]: it categorizes pull requests
  with GitHub labels rather than a commit convention, generates the exhaustive
  list from every merged pull request's title sorted by area label, and has
  authors write release notes only for pull requests labeled `M-Release-Note`.
  It works because Bevy's titles are written as plain descriptions (`Add drag to
  reorder tabs`).
  JP's are conventional-commit subjects written for developers, which is the
  problem this RFD exists to solve.
- **A configurable type vocabulary.** A workspace could sort its changes its own
  way, checked against a list in its configuration.
  But validation would then need configuration, and with it `jp`, and a fragment
  would mean different things in different workspaces.
  What a workspace does want to change, how a section reads on the page, is a
  heading, which [RFD 122] makes configurable.
  This flips when a workspace's changes do not sort into the seven types.
- **Breaking changes in the frontmatter**, as a boolean or as a string holding
  the upgrade text.
  A boolean leaves nowhere to say how to upgrade.
  A string makes the upgrade text YAML: no Markdown preview, no formatter, and
  awkward multi-line quoting, for the part of a release users read most
  carefully.
- **Follow-ups that edit the fragment they extend**, so a change and its
  follow-ups share one entry.
  Release notes would read slightly better, but a follow-up open while a release
  is cut would have its extension carried into the released copy by git,
  describing work the release does not ship, with nothing to notice it.
  Separate entries for one change can be merged into one while the release's
  notes are prepared ([RFD 122]).

## Non-Goals

- **Assembling and publishing releases from these inputs.** [RFD 122] adds the
  pipeline commands to `jp release`: release branches, release notes and upgrade
  guides, and jilu's retirement.
  This RFD stops at the input formats, the code that writes and validates them,
  and JP's CI.
- **Backfilling changelog entries for already-merged history.** The gate applies
  only from when it lands; changes merged earlier are not retroactively
  fragmented.
- **CI outside GitHub.** The gate and the label are JP's workflows; the plugin
  works without any CI.

## Risks and Open Questions

- **Fragment quality varies.** An entry can be accurate and still read like a
  commit message.
  The reviewer, and the `pr-reviewer` persona, are the backstop.
- **Highlight judgment is subjective.** "Worth a highlight" has no bright line.
  Start conservative; a release's introduction can carry a feature that does not
  get its own note.
- **First-release cutover.** Fragments exist only from when the gate lands, so
  the first release's grouped changelog covers post-cutover changes only.
  This is the cost of sourcing notes from pull requests rather than from git
  history.

## Implementation Plan

### Phase 1: Fragment format and CI

Create `crates/internal/changelog` with the fragment parser, the upgrade
section, validation (released versions' directories included), and the
`changelog` binary, and write `changelog.d/README.md`.
Add `just changelog-ci` to the task matrix in `rust.yml`, and add the gate
workflow with both of its rules.

This phase is the priority: contributors can write fragments by hand from the
day it merges, before the plugin exists.
Mergeable independently.

### Repository settings

These cannot land in a pull request; someone with admin access to the repository
applies them once Phase 1 has merged:

- Create the GitHub label `no-changelog`.
- Make `Changelog fragment or no-changelog label` a required status check on
  `main`.

Until then, the gate reports but does not block a merge.

### Phase 2: Highlight note format

Add `highlights/` to the parser and the validation.
Depends on Phase 1.

### Phase 3: The `jp release` plugin

Create `crates/plugins/command/release` with `init`, `check`, and `add`, and
admit it in `.jp/config.toml`.
`init` writes Phase 1's README into other workspaces.
Serve `add` as the `changelog_add` tool, with `tools init`, and enable it in the
`dev` and `rfd-implementor` personas.
Depends on Phase 1.

### Phase 4: Label auto-apply (optional)

The best-effort workflow that sets `no-changelog` by default.
Optional polish; the gate works with manually-applied labels without it.

The auto-apply uses a default path classification, documented in the fragments
README and overridable by a human on any pull request:

- **Shipping** (an entry is required by default): `crates/jp_*/**`,
  `crates/plugins/**`, and top-level runtime packaging files.
- **Non-shipping** (`no-changelog` by default): `docs/**`, `.github/**`,
  `.jp/**`, `.config/jp/tools/**`, `crates/internal/**`, `justfile`,
  `apps/macos/**` (the app is not distributed), `changelog.d/**` (such as a pull
  request that only corrects a released note), RFD drafts, and test fixtures.
- **Ambiguous** (default to requiring an entry; a human decides): top-level
  `Cargo.toml`, `Cargo.lock`, and `crates/contrib/**`.
- Any path not listed requires an entry by default.

Because the gate reads only the label, a misclassification is corrected by
toggling the label, never by editing CI.

## References

- [RFD 001: The JP RFD Process][RFD 001], the lifecycle and conventions this RFD
  follows.
- [RFD 077: Plugin Configuration and Trust Policy][RFD 077], the `run` and
  `options` settings the plugin is configured through.
- [RFD 116: Shareable RFD, Ticket, and Site Tooling][RFD 116], the plugin family
  `jp release` joins.
- [RFD 122: Release Pipeline][RFD 122], which turns these inputs into releases.
- [Keep a Changelog][keepachangelog], the categories the seven types adopt.
- [Conventional Commits][conventional-commits], the source of the `BREAKING
  CHANGE:` token and the `!` marker.
- [VRL's release tool][VRL], prior art for fragments and for crediting pull
  requests from commit subjects.
- [Bevy's release-content process][bevy-release] and its [release
  generator][bevy-generate-release], prior art for per-PR release notes,
  migration guides, and a changelog generated from pull-request titles.

[RFD 001]: 001-jp-rfd-process.md
[RFD 077]: 077-plugin-configuration-and-trust-policy.md
[RFD 116]: 116-shareable-rfd-ticket-and-site-tooling.md
[RFD 122]: 122-release-pipeline.md
[VRL]: https://github.com/vectordotdev/vrl/tree/main/release
[bevy-generate-release]: https://github.com/bevyengine/bevy-website/tree/main/generate-release
[bevy-release]: https://github.com/bevyengine/bevy/tree/main/_release-content
[conventional-commits]: https://www.conventionalcommits.org/en/v1.0.0/
[keepachangelog]: https://keepachangelog.com/
