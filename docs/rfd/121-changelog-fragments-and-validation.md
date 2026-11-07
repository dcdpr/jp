# RFD 121: Changelog Fragments and Validation

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-01
- **Extends**: [RFD 116]
- **Required by**: [RFD 122]
- **Summary**: Contributors record user-facing changes as typed fragments and
  highlight notes, written by a `jp release` plugin and required by CI.

## Summary

This RFD defines two plain Markdown formats under `changelog.d/` for recording
user-facing changes in pull requests: short per-change **changelog fragments**
grouped by impact type, whose filename holds the type, and editorial
**highlight notes** for major features, which open with their title.
A `jp release` command plugin creates and validates both formats in the user's
editor and exposes fragment creation as an assistant tool.
A CI check requires every pull request to either add a fragment or carry the
`no-changelog` label.
[RFD 122] covers assembling and publishing releases from these files.

## Motivation

JP is approaching its first official release without a structured way to record
user-facing changes.
Currently, `jilu` generates the changelog from Conventional Commit subjects in
git history, which is the wrong source for user-facing release notes.

Commit messages and release notes serve different audiences.
Commit messages explain code changes to developers; release notes explain new
capabilities and behavior changes to users.
For example, `fix(cli, md): Preserve trailing separator after terminal tight
list (#756)` is a precise commit subject, but a poor release note.
Generating release notes from commit subjects forces one line to serve both
audiences, serving neither well.

Writing release notes in the pull request captures context while it is fresh.
Storing each note as a separate file in `changelog.d/` also avoids the merge
conflicts caused by editing a single shared changelog file across concurrent
branches.

The first release draws the most attention and sets the template for future
releases.
To support both completeness and narrative from the start, the format provides
two tiers: terse, exhaustive changelog fragments and rich, editorial highlight
notes.

Like the ticket, RFD, and site tooling in [RFD 116], this workflow is useful to
any JP workspace.
The CLI commands and tools therefore ship as a `jp release` plugin in that
family, backed by a configuration-free library crate that external tools can
also use.

## Design

### Two input formats

Both formats are plain Markdown files stored in the fragments directory
(`changelog.d/`).
Anything besides their text is held in the filename or in a metadata block of
`- **Key**: value` lines, the same block RFDs and tickets use:

- A **changelog fragment** records one user-visible change per file.
  Fragments are terse, categorized by impact type, and exhaustive: every
  user-facing change requires one.
- A **highlight note** is a titled editorial section for a notable feature.
  Highlights are selective, written in prose, and can span multiple pull
  requests.

When cutting a release ([RFD 122]), JP compiles these files into release notes
(an introduction, highlight notes, and fragments grouped by type) and, if the
release contains breaking changes, an upgrade guide.
This RFD defines the two input formats, the crate and plugin that write and
validate them, and JP's CI checks, leaving release rendering to [RFD 122].

### Changelog fragment

A changelog fragment is a file placed directly in the fragments directory and
named `<slug>.<type>.md` (for example, `enable-decomposition.added.md`).
The file's text is the entry:

```markdown
`tools.<name>.enable` splits into `state` and `allow_toggle`, so you can pin a
tool on or off while still letting the assistant request the other state.
```

The type classifies **user impact** using one of seven fixed identifiers: the
six [Keep a Changelog][keepachangelog] categories plus `performance`.

| Type          | Used for                                                 |
| ------------- | -------------------------------------------------------- |
| `added`       | A new user-facing capability.                            |
| `changed`     | A change to existing behavior.                           |
| `deprecated`  | A feature that still works but is scheduled for removal. |
| `removed`     | A removed feature or capability.                         |
| `fixed`       | A user-visible bug fix.                                  |
| `security`    | A security-related fix.                                  |
| `performance` | A user-visible performance improvement.                  |

The type identifiers are fixed across all workspaces, so contributors use the
same vocabulary everywhere and format validation requires no configuration.
Release notes render the categories in the table order above, under section
headings that each workspace can customize ([RFD 122]).

Fragment types are distinct from Conventional Commit types (`feat`, `refactor`,
`test`, and so on).
Commit types classify the code change for developers, whereas fragment types
classify the effect on users.
The two often diverge: an internal `feat(tools)` has no user impact, while a
`refactor(cli)` might alter CLI behavior.
Changes without user impact, including most `build`, `ci`, `docs`, `refactor`,
and `test` work, produce no fragment.

Workspaces do not have to use Conventional Commits, though JP continues to use
them.
When a pull request title follows Conventional Commits, JP's CI uses the commit
type as a hint for the `no-changelog` label and requires an upgrade section when
the title includes `!` (see [CI validation](#ci-validation)).

The slug is descriptive and serves only to avoid collisions; the same slug can
name one fragment of each type.
The type is the last `.`-separated part of the filename before `.md`.
Keeping it in the filename rather than the file makes it visible in a directory
listing, and means a hand-written fragment needs nothing but its entry.
Because every fragment's filename ends in its type, no slug can collide with the
directory's `README.md` or a release's `overview.md`, in any letter case.
A metadata block at the top of the file may hold only `Credits` fields
([Credits](#credits)); it cannot name a type, since the filename holds it.

#### Breaking changes

When a change requires users to take action on upgrade, its fragment includes an
upgrade guide section starting with a `BREAKING CHANGE:` (or `BREAKING-CHANGE:`)
marker, as in `enable-explicit.removed.md`:

````markdown
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

A breaking-change fragment has three required, non-empty parts:

- **The entry**: all paragraphs before the marker.
  The release notes list the entry under its type's category, mark it as
  breaking, and link it to its upgrade section.
- **The title**: the text following `BREAKING CHANGE:` in the marker paragraph.
  This becomes the section heading in the upgrade guide ([RFD 122]) and should
  name what broke in terms a reader would search for.
- **The body**: all paragraphs and code blocks after the title, explaining what
  changed, why, and how to upgrade, with before-and-after examples where
  helpful.

A fragment may contain at most one upgrade section.
A line starting with either marker anywhere else, such as inside a code block or
partway through a paragraph, is a format error rather than text.
This keeps the gate's plain-text search for the marker consistent with the
parser ([CI validation](#ci-validation)): if the search finds a marker, the
fragment either has an upgrade section or fails validation.
To show the marker in an example, start the line with something else, such as
indentation or inline code.

Placing the upgrade section in the file's text rather than in a metadata field
lets authors write ordinary Markdown that renders, wraps, and formats alongside
the rest of the file.
The marker syntax is borrowed from Conventional Commits for familiarity, but
fragments do not require Conventional Commits to use it.
Highlight notes cannot carry an upgrade section; the corresponding changelog
fragment holds it.

#### Credits

Fragments do not record change request numbers by default (where a *change
request* is a forge's unit of review, such as a GitHub pull request or GitLab
merge request).
Authors often do not have a change request number when writing a fragment
locally, and backfilling it in CI would require a bot identity to push commits
to contributor branches.

Instead, the release pipeline ([RFD 122]) derives credits from git history on
`main`: it credits every change request whose commits added or modified the
fragment while it lived at the top level of the fragments directory.
Edits made after a fragment moves into a release directory
(`changelog.d/<version>/`) are treated as wording corrections and do not add
credits.
If git history names no change request, the entry credits the commit instead.
For the rare case where derived credits are inaccurate, `Credits` fields at the
top of the file override them, one per line.
Each names a change request on the workspace's forge, written `#1207` (or
`!1207`, as GitLab writes it), or a commit hash:

```markdown
- **Credits**: #1207
- **Credits**: 4e1f0a2

A turn that stalls while idle is retried instead of failing.
```

Every user-visible change adds a new fragment, including follow-up pull requests
that extend an earlier change, whether or not the earlier change has been
released.
Existing fragments are edited only to fix wording.
Because cutting a release moves top-level fragments into a version directory
([Released fragments](#released-fragments)), git's rename tracking carries any
in-flight edit from an open pull request into the released file when that pull
request merges.
That behavior is appropriate for wording fixes, but would attribute unreleased
work to an already-cut release if a follow-up appended new functionality to an
existing fragment.
A feature built across multiple pull requests instead adds a separate fragment
in each pull request with user-visible changes and updates a single shared
highlight note, which credits all of them.

### Highlight note

A highlight note is a file in `highlights/` under the fragments directory, named
`<slug>.md`, for a feature worth an editorial section in the release notes.
It opens with its title, followed by a metadata block crediting its authors:

```markdown
# Unified editor and inline reply widget

- **Authors**: @JeanMertz

Every prompt in JP now runs through a single editor service, and short replies
can be written inline without leaving the conversation view. Longer edits still
open your editor, so the two paths share one backend and one set of keybindings.
```

- The title, a `# Title` heading on the first line, is the section heading in
  the release notes.
- `Authors`: a contributor credited for the feature, one per field; at least one
  is required.
- `Credits` (optional, after the authors): explicit change requests or commits
  that replace the git-derived credits.
  By default, a highlight note is credited the same way as a fragment, to every
  change request that added or modified it, so a note updated across several
  pull requests automatically credits each of them.
  Use `Credits` when contributing change requests did not touch the highlight
  file.

Highlight notes are selective and editorial.
Most changes get only a changelog fragment; highlights are reserved for major
features that warrant a narrative.
A highlighted change still requires a changelog fragment so that it also appears
in the exhaustive categorized list.
Unlike fragments, highlight notes have no type and no upgrade section.

A highlight note must always reflect the current state of `main`.
Because releases are cut from `main` on a schedule, any commit on `main` can end
up in a release, and incremental work cannot leave a highlight describing
unmerged behavior.
A feature that is still hidden gets its highlight note in the pull request that
makes it visible to users.
A feature that is visible but incomplete gets a highlight describing what
currently exists, which later pull requests expand as more pieces land.

### Released fragments

Cutting a release ([RFD 122]) opens a pull request against `main` that moves the
unreleased fragments and highlight notes from the top level of `changelog.d/`
into a subdirectory named after the release version.
Backporting a later change into a release moves that change's fragment into the
version directory in the same way:

```
changelog.d/
  README.md
  enable-explicit.removed.md    # unreleased
  highlights/fan-out.md         # unreleased
  0.3.0/
    overview.md
    retry-idle-turns.fixed.md
    highlights/plugin-tools.md
```

Files in a version directory belong to that release, while top-level files are
unreleased ([RFD 122] determines which release ships them).
Released fragments remain on `main` permanently.
All fragment additions, moves, and corrections happen exclusively on `main`;
release branches contain only code and have no `changelog.d/` ([RFD 122]).
Released and unreleased files share the same format, and validation checks
released directories as well, so a wording fix to an older release's notes is
validated like any new change.
A version directory may also contain `overview.md`, the release's introduction
([RFD 122]).

### Crates and the `jp release` plugin

The implementation is split across two crates, matching how [RFD 116] separates
the ticket format crate from `jp ticket`:

- **`crates/internal/changelog`** is the core library.
  It parses and validates both formats, and [RFD 122] adds crediting and
  rendering to it.
  It reads no configuration and does not depend on JP's plugin protocol;
  project-specific settings are passed as arguments so external release or
  changelog tools can reuse the crate.
  It also builds a small `changelog` binary that validates a fragments directory
  in CI.
- **`crates/plugins/command/release`** is the `jp release` command plugin (a
  library with a thin `main` binary built as `jp-release`).
  It is configured through `plugins.command.release.options` and installed in
  the same `cargo install` step as the [RFD 116] plugins so they share a
  revision.
  It handles host integration: CLI commands, configuration loading, opening
  files in the user's editor, and assistant tools.

Configuration in `.jp/config.toml`:

```toml
[plugins.command.release]
run = "allow"

[plugins.command.release.options]
fragments_dir = "changelog.d"
format_on_write = true
```

`fragments_dir` is optional and defaults to `changelog.d`, relative to the
workspace root.
`format_on_write` is optional and defaults to `false`.
When enabled, every file the plugin writes is formatted according to the
workspace's `.comfort.toml`, matching how the [RFD 116] plugins format their
documents.
Setting `run = "allow"` permits the plugin to run without prompting for approval
([RFD 077]), which avoids repeated prompts when rebuilding the plugin from the
local checkout.

| Command                       | Description                                                                               |
| ----------------------------- | ----------------------------------------------------------------------------------------- |
| `jp release init`             | Creates the fragments directory, `highlights/`, and a `README.md` describing both formats |
| `jp release <type> [SLUG]`    | Opens the fragment `<slug>.<type>.md` in the user's editor, creating it if needed         |
| `jp release highlight [SLUG]` | Opens a new highlight note, or `highlights/<slug>.md`, in the user's editor               |
| `jp release check`            | Validates every file in the fragments directory (see [CI validation](#ci-validation))     |

Each type is its own command, such as `jp release fixed`, so contributors name
the type the way they would describe the change: "I fixed this".
The slug defaults to the git branch name (the part after its last `/`), so
running the command again on the same branch reopens the same fragment.
On `main`, whose name describes no change, the command asks for a slug.
The editor is the host's `editor.cmd`, opened through the plugin protocol's
`edit_files` request.
When the editor closes, the file is validated with the same check as `jp
release check`, both as written and as formatted.
A file that fails validation reopens in the editor, and abandoning it undoes the
change.
A fragment saved empty is removed.
A new highlight note opens with a `# Title` line to fill in and the user
credited as its author, and is named after its title once saved; `jp release
highlight <slug>` opens `highlights/<slug>.md` instead.

Writing a fragment is also exposed as a plugin tool, `add`, configured in JP like
`ticket_create`:

```toml
[conversation.tools.changelog_add]
source = "plugin.command.release.add"
style.parameters = "tool"
```

The `changelog_add` tool writes fragments only, not highlight notes.
Highlight notes are multi-paragraph editorial prose shaped by humans, and the
assistant edits them directly as Markdown files.
Setting `style.parameters = "tool"` delegates parameter rendering to the plugin,
which previews the fragment before writing it.
Because the tool's `type` parameter uses the fixed list of seven identifiers,
its schema does not depend on workspace configuration: `jp release tools init`
writes the tool definition once, no `tools sync` command is needed, and (as in
[RFD 116]) `tools init` will be removed once plugins can declare their own tool
schemas directly.
JP enables `changelog_add` in the personas that implement changes (`dev` and
`rfd-implementor`), and the `pr-reviewer` persona checks that each fragment
describes the change from the user's perspective.

### CI validation

JP's CI enforces three rules:

1. **Format**: every file must follow its format, and unknown metadata fields
   are rejected.

   A fragment's filename must end in one of the seven type identifiers,
   `<slug>.<type>.md`.
   An optional metadata block at the top may hold only `Credits` fields, each a
   change request (`#1207`, `!1207`) or a commit hash of at least seven
   characters.
   The entry after it must be non-empty after trimming surrounding whitespace,
   and any upgrade section must follow the structure in [Breaking
   changes](#breaking-changes).

   A highlight opens with a `# Title` heading with a title, then a metadata
   block holding at least one `Authors` field, each naming someone, and any
   number of `Credits` fields, and then non-empty prose.

   Unknown metadata fields are rejected rather than ignored so that typos like
   `- **Credit**:` fail in CI instead of being silently dropped.
   A `Type` field is rejected as well: the filename holds the type, and a second
   place to write it could disagree with the first.

2. **Presence**: a pull request targeting `main` passes only if it adds a new
   fragment at the top level of the fragments directory or carries the
   `no-changelog` GitHub label.
   Modifying an existing fragment, adding only a highlight note, or editing a
   released fragment does not satisfy this rule:

   - Editing an existing fragment only corrects wording ([Credits](#credits)).
   - Adding a highlight note without a fragment would omit the change from the
     categorized changelog list.
   - Editing a released fragment updates historical release notes without
     changing the software.

   Pull requests limited to those edits must carry `no-changelog`.
   The rule checks only for a newly added top-level fragment or the label;
   whether a change is user-facing is decided when applying the label (see the
   auto-apply step below), not computed by the gate.

3. **Breaking change**: a pull request targeting `main` whose title marks a
   breaking change with `!` (for example, `feat(cli)!: ...`) fails unless at
   least one fragment it adds contains an upgrade section, or the pull request
   carries `no-changelog` (for breaking changes to internal interfaces users
   never see).
   Projects that do not use Conventional Commits never trigger this rule.

The set of paths allowed in the fragments directory is closed:

- `README.md` at the top level, which documents both formats and is ignored by
  the parser (`highlights/` has no `README.md`).
- `<slug>.<type>.md` and `highlights/<slug>.md` at the top level.
- `<version>/<slug>.<type>.md`, `<version>/highlights/<slug>.md`, and
  `<version>/overview.md`, where `<version>` is a semantic version directory.

Any other path is a CI error rather than a silently ignored file, failing early
instead of vanishing during release assembly.
This rejects a fragment without a type in its filename, other file extensions
(`.txt`, `.markdown`), a misspelled `overview.md`, non-version subdirectories,
deeper directory nesting, or stray files under `highlights/`.
A top-level `overview.md` has no type in its filename, so it is an error rather
than a release's introduction.

CI runs these rules as two separate checks because they have opposite triggering
requirements.

**Format validation** runs via `just changelog-ci`, a task in the `rust.yml`
matrix that builds and executes the standalone `changelog` binary.
Because fragment type identifiers are fixed, validation requires no
configuration and no `jp` binary: the task builds only the `changelog` crate.
The binary takes the fragments directory path as an argument (defaulting to
`changelog.d`), while `jp release check` calls the same library function with
the configured `fragments_dir`.
Like other matrix tasks, `changelog-ci` runs on pushes to `main` and on pull
requests touching `changelog.d/**` or `crates/internal/changelog/**` (so parser
changes revalidate existing files).
The path filter skips the task's steps, not its job, so the job reports a status
on every pull request.
JP configures it as a required status check as well ([Repository
settings](#repository-settings)), so a fragment that fails validation cannot
merge, which the gate relies on.

**The changelog gate** enforces the presence and breaking-change rules and is
configured in branch protection as a required status check on `main`.
It runs on every pull request without path filters: a required check that is
skipped never reports a status, blocking the merge indefinitely.
It cannot live in the `rust.yml` matrix because it must re-run whenever a pull
request's labels or title change.
GitHub Actions cannot filter `labeled`, `unlabeled`, and `edited` events by
changed paths, so adding those triggers to `rust.yml` would re-run (and, off
`main`, cancel) the entire test matrix on every label event.
Instead, the gate runs in its own lightweight workflow triggered on `opened`,
`synchronize`, `reopened`, `labeled`, `unlabeled`, and `edited`, with a
dedicated concurrency group.
It requires no Rust toolchain.
It reads the title from the pull request event payload and the labels from the
REST API at run time, so a re-run sees the current labels rather than those in
the triggering event.
It checks for added top-level fragments (excluding `README.md`) with `git diff
--diff-filter=A` against the base branch, and checks added fragments for the
`BREAKING CHANGE:` marker with `grep`, which the format rule keeps consistent
with the parser ([Breaking changes](#breaking-changes)).
Label and title edits re-run only this workflow, completing in seconds.

Because GitHub's pull request check list shows only a job's name and status, the
gate surfaces failures in two places: its job name (`Changelog fragment or
no-changelog label`) states the requirement directly in the check list, and a
failure emits an error annotation on the check page with the fix (for example,
"Add a fragment, such as with `jp release fixed`, or add the `no-changelog`
label.").

The gate applies only to pull requests targeting `main`; pushes to `main` and
local `jp release check` runs perform format validation only.
Pull requests opened by `jp release` to cut a release or backport a change ([RFD
122]) move existing fragments rather than adding new ones, and pass the gate via
the `no-changelog` label that `jp release` attaches to them.
Release branches receive commits directly from `jp release`, never via pull
requests, and have no `changelog.d/` directory, so neither check runs on them.
Fork pull requests work the same way, since reading labels needs no write
access.
Only the auto-apply step below needs write access, and it skips pull requests
from forks, where a maintainer applies the label manually.

A best-effort step in the gate, run before the gate evaluates, sets
`no-changelog` automatically as an overridable default:

- When a pull request opens, the step applies `no-changelog` if the Conventional
  Commit type in the title is non-user-facing (`build`, `ci`, `docs`,
  `refactor`, `test`), or if the diff touches only non-shipping paths
  (documentation, CI, internal tooling).
  It never auto-applies the label for user-facing commit types (`feat`,
  `enhance`, `fix`, `perf`).
  If the title has no Conventional Commit prefix, or uses a type outside both
  lists (such as `chore`, which is used in practice despite being absent from
  the documented taxonomy), the step falls back to path classification alone.
- Manual changes always take precedence: the step acts only when a pull request
  opens, so once a human adds or removes `no-changelog`, that choice is final.

The auto-apply logic is a step in the gate rather than a separate workflow
because a label added with a workflow's token does not trigger other workflows:
a separate workflow would label the pull request while the gate, which ran when
the pull request opened, stayed red.

The gate intentionally reads the `no-changelog` label rather than the pull
request title because commit types are an imperfect predictor of user impact.
Using the label keeps that judgment human-overridable, and the gate works
without the auto-apply step.

The gate, the `no-changelog` label, and the auto-apply step are specific to JP's
GitHub repository and are not part of the plugin.
Workspaces without CI or on other forges can use `jp release` without them.

## Drawbacks

- **An extra artifact in every user-facing pull request.** Contributors must
  write a fragment and reviewers must check its prose.
  This is an intentional cost, but adds friction that did not exist before.
- **A changelog decision on every pull request.** Because the gate is a required
  status check, even a one-line chore requires the `no-changelog` label before
  merging, until the auto-apply step handles it automatically.
- **Two formats to learn.** Mitigated because most pull requests only need a
  terse changelog fragment; highlight notes are rare and reserved for notable
  features.
- **Auto-apply label management is GitHub-specific and fork-limited.** It is
  best-effort polish, and the gate works without it.

## Alternatives

- **Keep `jilu` and git history as the changelog source.** Rejected because it
  conflates the developer and user audiences, which is the core problem this RFD
  solves.
- **Edit a single `CHANGELOG.md` in each pull request.** Rejected because it
  causes merge conflicts across concurrent branches.
  Per-change fragment files are the standard solution.
- **Store the pull request number in the fragment, backfilled by CI.** Rejected
  because it requires a bot identity and CI commits to contributor branches.
  Deriving credits from git history avoids both.
- **Use `towncrier` (Python).** Rejected to avoid introducing a Python toolchain
  dependency for logic that fits in a few hundred lines of Rust.
- **Single-tier fragments only (no highlight notes).** Rejected because the
  first official release warrants editorial prose, and deferring highlight notes
  would leave the most visible release without them.
- **Use the Conventional Commit type as the fragment type.** Every pull request
  title already has a commit type, but commit types classify the code change
  rather than its effect on users, and a single pull request has one title type
  even when it makes multiple user-visible changes.
  Prior art such as [VRL] uses Conventional Commit pull request titles (its
  release tool landed as `chore(releasing): ...`) while ignoring them for the
  changelog, encoding the fragment type in the fragment filename instead.
- **Generate the changelog from pull request titles, with opt-in highlight
  notes.** [Bevy's release process][bevy-release] categorizes pull requests with
  GitHub labels rather than commit prefixes, generates the exhaustive list from
  merged pull request titles sorted by area label, and collects prose notes only
  for pull requests labeled `M-Release-Note`.
  That works because Bevy's pull request titles are written as user-facing
  descriptions (`Add drag to reorder tabs`), whereas JP's titles are
  Conventional Commit subjects written for developers.
- **Make the fragment type vocabulary configurable.** Allowing workspaces to
  define custom types in configuration would require validation to load
  configuration (and depend on `jp`), and fragments would no longer share fixed
  semantics across workspaces.
  What workspaces want to customize is the rendered section heading on the
  release page, which [RFD 122] makes configurable.
  This decision can be revisited if a workspace's changes do not fit the seven
  fixed types.
- **Record breaking changes in a metadata field** (as a flag or as the upgrade
  text).
  A flag provides nowhere to explain how to upgrade, and a field's value is a
  single line of plain text, while the section of a release users read most
  carefully needs paragraphs and code blocks.
- **Store metadata in YAML frontmatter**, including `type`, between `---` lines.
  Every fragment would carry a YAML block for a single field, which contributors
  writing by hand must get right, and author handles starting with `@` need
  quoting.
  The type in the filename is one word that shows in a directory listing, and
  the metadata block RFDs and tickets already use holds the rare `Credits`.
- **Edit existing fragments in follow-up pull requests** so a change and its
  follow-ups share one entry.
  Release notes would read slightly cleaner without manual consolidation, but if
  a release is cut while a follow-up pull request is open, git's rename tracking
  carries the follow-up's edit into the moved file in `changelog.d/<version>/`,
  attributing unshipped work to the release.
  Separate fragments for related changes can instead be combined while preparing
  the release notes ([RFD 122]).

## Non-Goals

- **Assembling and publishing releases from these inputs.** [RFD 122] adds the
  release pipeline commands to `jp release`: release branches, release note and
  upgrade guide rendering, and retiring `jilu`.
  This RFD stops at the input formats, the code that writes and validates them,
  and JP's CI checks.
- **Backfilling changelog entries for already-merged history.** The gate applies
  prospectively from when it lands; earlier commits are not retroactively
  converted into fragments.
- **CI integration outside GitHub.** The gate and label workflows target JP's
  GitHub repository; the `jp release` plugin works without any CI.

## Risks and Open Questions

- **Fragment quality varies.** A fragment can be factually accurate and still
  read like a developer-focused commit message.
  Human reviewers and the `pr-reviewer` persona serve as the backstop.
- **Highlight selection is subjective.** There is no strict rule for what
  warrants a highlight note.
  Contributors should start conservatively; notable features without a dedicated
  highlight note can still be covered in the release's `overview.md`
  introduction.
- **First-release cutover.** Because fragments exist only after the gate lands,
  the first release's categorized changelog will cover only post-cutover
  changes.
  This is an accepted trade-off of sourcing release notes from pull requests
  rather than git history.

## Implementation Plan

### Phase 1: Fragment format and CI

Create `crates/internal/changelog` with the fragment parser, upgrade section
parser, directory and schema validation (including released version
directories), and the `changelog` binary, and write `changelog.d/README.md`.
Add `just changelog-ci` to the task matrix in `rust.yml`, and add the changelog
gate workflow enforcing the presence and breaking-change rules.

This phase is the top priority and can merge independently: contributors can
write fragments by hand as soon as it lands, before the plugin exists.

### Repository settings

A repository administrator applies three settings once Phase 1 has merged (these
cannot be changed in a pull request):

- Create the `no-changelog` GitHub label.
- Configure `Changelog fragment or no-changelog label` as a required status
  check on `main`.
- Configure the `changelog` task of the `rust` workflow as a required status
  check on `main`, so a fragment that fails format validation cannot merge.

Until then, both checks report status on pull requests without blocking merges.

### Phase 2: Highlight note format

Add `highlights/` parsing and validation to `crates/internal/changelog`.
Depends on Phase 1.

### Phase 3: The `jp release` plugin

Create `crates/plugins/command/release` with `init`, `check`, a command per
type, and `highlight`, and allow it in `.jp/config.toml`.
`init` writes the `README.md` template from Phase 1 into other workspaces.
Expose writing a fragment as the `changelog_add` tool via `jp release tools
init`, and enable it in the `dev` and `rfd-implementor` personas.
Depends on Phase 1.

### Phase 4: Label auto-apply (optional)

Add the gate's best-effort step that applies `no-changelog` by default when pull
requests open.
This phase is optional polish; the gate works with manually applied labels
without it.

The step uses a default path classification, documented in
`changelog.d/README.md` and overridable by a human on any pull request:

- **Shipping** (requires a fragment by default): `crates/jp_*/**`,
  `crates/plugins/**`, and top-level runtime packaging files.
- **Non-shipping** (`no-changelog` by default): `docs/**`, `.github/**`,
  `.jp/**`, `.config/jp/tools/**`, `crates/internal/**`, `justfile`,
  `apps/macos/**` (the app is not distributed), `changelog.d/**` (such as a pull
  request that only corrects a released note), RFD drafts, and test fixtures.
- **Ambiguous** (requires a fragment by default; a human decides): top-level
  `Cargo.toml`, `Cargo.lock`, and `crates/contrib/**`.
- Any path not listed requires a fragment by default.

Because the gate reads only the label, any misclassification is fixed by
toggling `no-changelog` on the pull request rather than editing CI.

## References

- [RFD 001: The JP RFD Process][RFD 001]: the lifecycle and conventions this RFD
  follows.
- [RFD 077: Plugin Configuration and Trust Policy][RFD 077]: the `run` and
  `options` settings used to configure the plugin.
- [RFD 116: Shareable RFD, Ticket, and Site Tooling][RFD 116]: the plugin family
  that `jp release` joins.
- [RFD 122: Release Pipeline][RFD 122]: release assembly, crediting, and
  publishing from these inputs.
- [Keep a Changelog][keepachangelog]: the categories adopted by the seven
  fragment types.
- [Conventional Commits][conventional-commits]: the source of the `BREAKING
  CHANGE:` marker and the `!` breaking-change indicator.
- [VRL's release tool][VRL]: prior art for changelog fragments and deriving pull
  request credits from commit subjects.
- [Bevy's release-content process][bevy-release] and its [release
  generator][bevy-generate-release]: prior art for per-PR release notes,
  migration guides, and changelogs generated from pull request titles.

[RFD 001]: 001-jp-rfd-process.md
[RFD 077]: 077-plugin-configuration-and-trust-policy.md
[RFD 116]: 116-shareable-rfd-ticket-and-site-tooling.md
[RFD 122]: 122-release-pipeline.md
[VRL]: https://github.com/vectordotdev/vrl/tree/main/release
[bevy-generate-release]: https://github.com/bevyengine/bevy-website/tree/main/generate-release
[bevy-release]: https://github.com/bevyengine/bevy/tree/main/_release-content
[conventional-commits]: https://www.conventionalcommits.org/en/v1.0.0/
[keepachangelog]: https://keepachangelog.com/
