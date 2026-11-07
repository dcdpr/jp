# RFD 041: RFD Lifecycle Enhancements

- **Status**: Implemented
- **Category**: Process
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-09
- **Extended by**: [RFD 100]
- **Summary**: Defers RFD numbering until Discussion, adds `Extends` and
  `Extended by` metadata fields, and automates implementation tracking at
  Acceptance.

## Summary

This RFD makes three changes to the RFD lifecycle: it defers RFD numbering until
Discussion (instead of Draft creation), creates a GitHub tracking issue
automatically when an RFD reaches Accepted, and adds `Extends` and `Extended by`
metadata fields to make directional relationships between RFDs machine-readable.

[RFD 100] supersedes the tracking-issue mechanism, tracking implementation work
in tickets instead.

## Motivation

**Numbers are assigned too early.** `rfd-draft` assigns a permanent number on
creation, creating a stable reference before the RFD is reviewed or finished:

- Splitting a draft into two focused RFDs requires renumbering, which breaks any
  in-flight references.
- Authors pre-assign numbers to related RFDs they intend to write, leaving gaps
  and ordering confusion if any of those drafts are abandoned.
- An RFD number should signal identity and commitment, not just that someone
  started typing.

**Implementation tracking lives in the wrong place.** The Implementation Plan
section of a design RFD lists phases and steps, but nothing tracks completed
phases, links to the PRs that implemented them, or collects notes during
development.
That context ends up scattered across commit messages and PR descriptions, or
goes unrecorded.

**RFD relationships are invisible to tooling.** When an RFD extends an earlier
design, only prose and TIP callouts record the relationship.
Because the metadata header and index omit it, tooling cannot query it and
readers must discover it by reading the documents.

## Design

### 1. Deferred Numbering

Drafts are created as `NNN-title.md`, using the literal placeholder `NNN`
instead of a number in the filename and `# RFD NNN: Title` heading, and carry no
number in their metadata.

When the author runs `just rfd-promote NNN-title` to advance a draft to
Discussion, the tooling:

1. Assigns the next available sequential number.
2. Renames the file (for example, `NNN-title.md` to `041-title.md`).
3. Updates the `# RFD NNN: Title` heading in the file.
4. Proceeds with the existing promotion flow (status → Discussion).

An RFD gains a stable, linkable number only at Discussion.
Because drafts have no number, other RFDs cannot link to them by number, which
eliminates speculative cross-draft dependencies and forces authors to resolve
design relationships before both RFDs reach Discussion.

### 2. Tracking Issues

> [!TIP]
> [RFD 100] supersedes this section.
> `jp rfd promote` offers to file a tracking ticket labeled `tracking-rfd=NNN`
> with the Implementation Plan as its body, and that ticket is both the tracking
> item and the prioritized work item.
> Promotion no longer creates a GitHub tracking issue, and the `Tracking Issue`
> metadata field is retired.
>
> The original text below is preserved for historical context.
> Note that parts of this RFD describe issue creation at Discussion, while the
> implementation created it at Accepted.

When `just rfd-promote` advances an RFD to Accepted, it automatically creates a
GitHub tracking issue alongside the status change.

The issue:

- Links to the RFD document.
- Contains a task list generated from the RFD's Implementation Plan by
  `rfd_open_tracking_issue`, an LLM-assisted tool in the `rfd` skill that
  derives a structured task list from the RFD rather than filling a static
  template.
- Holds implementation notes, PR links, and progress tracking.

Promotion adds a `Tracking Issue: #XXX` line to the metadata header:

```markdown
- **Status**: Discussion
- **Category**: Design
- **Authors**: ...
- **Date**: ...
- **Tracking Issue**: #XXX
```

**Scope by category:**

| Category | Tracking issue                        |
| -------- | ------------------------------------- |
| Design   | Always                                |
| Decision | Always                                |
| Guide    | Only if the guide describes a process |
|          | with implementation steps             |
| Process  | Only if the process change has        |
|          | implementation steps                  |

**Lifecycle:** Abandoning an RFD closes its tracking issue with a comment
stating why.
Superseding an RFD closes its tracking issue and links it to the successor's
tracking issue.

### 3. `Extends` and `Extended by` Metadata

Two optional metadata fields capture directional extension relationships between
RFDs:

```markdown
- **Extends**: RFD 028
- **Extended by**: RFD 034, RFD 037
```

`Extends` appears in the newer RFD and `Extended by` in the older one.
Running `just rfd-extend NNN MMM` updates both files atomically, writing
`Extended by: MMM` into NNN and `Extends: NNN` into MMM.

The relationship is n-to-m: an RFD can extend multiple predecessors and be
extended by multiple successors, so both fields accept comma-separated RFD
numbers.

**Distinction from `Supersedes`:** A superseding RFD replaces its predecessor,
retiring the old design.
An extending RFD builds on its predecessor while leaving the original design in
effect.
For example, RFD 034 extends RFD 028 by adding cheaper model routing rather than
superseding the inquiry mechanism itself.

**Constraint:** These fields reference only RFDs in Accepted status or later.
Deferred numbering enforces part of this automatically: a draft has no number,
so it cannot appear in an `Extends` or `Extended by` field.

### 4. Related RFDs (Automated)

Rather than introducing a manual `Related` metadata field whose bidirectional
links are costly to maintain at scale, the VitePress site loader scans each RFD
document for links to other RFDs and surfaces them automatically.
Individual RFD pages display a "Referenced by" list, and the RFD index replaces
its `Date` column with relationship indicators.

## Drawbacks

- **Habit change.** Contributors are used to getting a number immediately, and
  circulating drafts can no longer be referenced by number.
  That constraint is intentional, but it requires adjustment.

- **`rfd-promote` is a larger operation.** Instead of changing one status line
  in one file, promotion renames a file, updates a heading, creates a GitHub
  issue, and updates metadata.
  The operation must be atomic: if GitHub issue creation fails, the file rename
  must not persist.

- **Tracking issues add noise to the GitHub issue tracker.** A dedicated `rfd`
  label and clear issue titles (`RFD-041: ...`) mitigate this by making tracking
  issues easy to filter.

## Alternatives

### Assign numbers at Accepted, not Discussion

Deferring numbering until Accepted would assign numbers only after consensus is
reached.
This was rejected because external references start at Discussion: reviewers
link to the RFD in comments, other authors reference it in their own RFDs, and
tracking issues need a stable title.

### `Related` as manual metadata

An author-maintained `Related: RFD NNN, RFD MMM` field was rejected because
maintaining bidirectional links by hand grows more costly with every new RFD.
Scanning document links in the site loader covers the same use cases
automatically.

### Static tracking issue template

Using a fixed template (boilerplate plus a link to the RFD) for all tracking
issues was rejected because implementation phases vary across RFDs: a five-phase
Design RFD needs five checkboxes, while a Decision RFD may need none.
`rfd_open_tracking_issue` reads the Implementation Plan and generates the task
list to match.

## Non-Goals

- **Renumbering existing drafts.** Existing Draft-status RFDs keep their numbers
  under the grandfather clause.
- **Automated tracking issue updates.** Promotion creates the tracking issue
  once; keeping it current by checking off tasks is the author's and
  implementer's responsibility.
- **`Related` as manual metadata.** The VitePress site loader derives
  related-RFD links automatically.
- **Changes to the Discussion or Accepted lifecycle steps** beyond what is
  described above.

## Implementation Plan

### Phase 1: Deferred numbering tooling

Update `just rfd-draft` to create `NNN-title.md` with the literal placeholder
`NNN` in the filename, heading, and metadata.
Update `just rfd-promote` to detect Draft RFDs (`NNN-*.md`), assign the next
available number, rename the file, and update the heading before changing the
status.
Document the policy in `docs/rfd/001-jp-rfd-process.md`.

Depends on: nothing.
Can be merged independently.

### Phase 2: Tracking issue creation

Add `rfd_open_tracking_issue` to the `rfd` skill in `.jp/config/skill/rfd.toml`.
The tool takes the RFD number, title, document URL, and an array of task strings
derived from the Implementation Plan, creates a GitHub issue via `gh issue
create` (or `curl` against the GitHub API), and returns the issue URL.

Extend `just rfd-promote` to call this tool when advancing to Discussion and
write the returned issue number into the `Tracking Issue:` metadata field.
If issue creation fails, abort promotion before renaming the file.
Document the `Tracking Issue` field and its abandon/supersede lifecycle in
`docs/rfd/001-jp-rfd-process.md`.

Depends on: Phase 1 (the promotion command is extended, not replaced).

### Phase 3: `Extends` / `Extended by` metadata and `rfd-extend` command

Add `just rfd-extend NNN MMM` to the justfile to write `Extended by: RFD MMM`
into `NNN`'s metadata and `Extends: RFD NNN` into `MMM`'s metadata, appending to
existing comma-separated lists rather than replacing them.
Update the metadata header documentation and tooling table in
`001-jp-rfd-process.md`.

Depends on: nothing.
Can be merged independently of Phases 1 and 2.

### Phase 4: Website automation for related RFDs

Extend the VitePress data loader (`docs/.vitepress/loaders/rfds.data.js`) to
parse each RFD document for Markdown links matching `NNN-*.md`, expose the
referenced RFD numbers as `references` on each RFD data object, and compute
`referenced_by` as the inverse.
Update the index page to replace the `Date` column with a references indicator,
and surface `referenced_by` on individual RFD pages.

Depends on: nothing.
Can be merged independently.

## References

- [RFD 001: The JP RFD Process][RFD 001]: the process this RFD extends.
- [RFD 003: JP-Assisted RFD Writing][RFD 003]: defines the `rfd` skill that
  gains the `rfd_open_tracking_issue` tool.

[RFD 001]: 001-jp-rfd-process.md
[RFD 003]: 003-jp-assisted-rfd-writing.md
[RFD 100]: 100-in-repo-ticket-tracking.md
