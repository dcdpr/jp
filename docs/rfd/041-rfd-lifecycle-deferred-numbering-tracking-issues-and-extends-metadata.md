# RFD 041: RFD Lifecycle: Deferred Numbering, Tracking Issues, and Extends Metadata

- **Status**: Implemented
- **Category**: Process
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-09

## Summary

This RFD introduces three improvements to the RFD lifecycle: RFD numbers are
assigned at Discussion (not at Draft creation), a GitHub tracking issue is
created automatically at that same promotion event, and two new metadata fields
(`Extends` / `Extended by`) make directional relationships between RFDs
machine-readable.

## Motivation

Three independent problems with the current process motivated these changes.

**Numbers are assigned too early.** Today, `rfd-draft` assigns a permanent
number on creation. That number becomes a stable reference before the RFD has
been reviewed or even finished. This means:

- Splitting a draft into two focused RFDs requires renumbering, which breaks any
  in-flight references.
- Authors pre-assign numbers to related RFDs they intend to write, creating gaps
  and ordering confusion if any of them are abandoned.
- The RFD number signals identity and commitment, but it currently signals
  nothing more than "someone started typing."

**Implementation tracking lives in the wrong place.** The Implementation Plan
section of a design RFD lists phases and steps, but there is no mechanism to
track which phases have been completed, link to the PRs that implemented them,
or collect the implementation notes that accumulate during development. That
information ends up scattered across commit messages, PR descriptions, and
nowhere.

**RFD relationships are invisible to tooling.** When a later RFD extends an
earlier design — adding a capability, improving a mechanism, or building on the
foundation — that relationship is captured only in prose and TIP callouts. It is
not in the metadata header, not in the index, and not queryable. The
relationship has to be discovered by reading.

## Design

### 1. Deferred Numbering

Drafts are created as `NNN-title.md` — the literal placeholder `NNN`, not a
number. The file header uses `NNN` in the title and carries no number in the
metadata.

When the author runs `just rfd-promote NNN-title` to advance to Discussion, the
tooling:

1. Assigns the next available sequential number.
2. Renames the file from `NNN-title.md` to `041-title.md`.
3. Updates the `# RFD NNN: Title` heading in the file.
4. Proceeds with the existing promotion flow (status → Discussion).

From that point the RFD has a stable identity. Other RFDs may reference it by
number. Before that point, no number exists to reference.

**What this enforces:** An RFD in Draft status cannot be linked to by number
from another RFD, because it does not have one. This eliminates speculative
cross-draft dependencies and forces design relationships to be resolved before
both RFDs reach Discussion.

### 2. Tracking Issues

When `just rfd-promote` advances an RFD to Discussion, a GitHub tracking issue
is created automatically alongside the file rename and status change.

The issue:

- Links to the RFD document.
- Contains a task list generated from the RFD's Implementation Plan by the
  `rfd_open_tracking_issue` tool (an LLM-assisted tool in the `rfd` skill that
  reasons about the RFD and produces a structured task list; the task list is
  not a static template).
- Serves as the canonical place for implementation notes, PR links, and progress
  tracking.

A `Tracking Issue: #XXX` line is added to the metadata header:

```markdown
- **Status**: Discussion
- **Category**: Design
- **Authors**: ...
- **Date**: ...
- **Tracking Issue**: #XXX
```

**Scope by category:**

| Category | Tracking issue                           |
|----------|------------------------------------------|
| Design   | Always                                   |
| Decision | Always                                   |
| Guide    | Only if the guide describes a process    |
|          | with implementation steps                |
| Process  | Only if the process change has           |
|          | implementation steps                     |

**Lifecycle:** When an RFD is abandoned, the tracking issue is closed with a
comment stating why. When an RFD is superseded, the tracking issue is closed
and linked to the successor's tracking issue.

### 3. `Extends` and `Extended by` Metadata

Two new optional metadata fields capture directional extension relationships
between RFDs:

```markdown
- **Extends**: RFD 028
- **Extended by**: RFD 034, RFD 037
```

`Extends` is written in the newer RFD. `Extended by` is written in the older
one. Both are updated by `just rfd-extend NNN MMM`, which writes `Extended by:
MMM` into NNN and `Extends: NNN` into MMM atomically.

The relationship is n-to-m: an RFD can extend multiple predecessors, and can be
extended by multiple successors. Both fields accept comma-separated RFD numbers.

**Distinction from `Supersedes`:** A superseding RFD replaces its predecessor —
the old design is no longer the current approach. An extending RFD builds on its
predecessor — the original remains valid and in effect. RFD 034 extends RFD 028
(adds cheaper model routing); it does not supersede it (the inquiry mechanism is
still the approach, just improved).

**Constraint:** These fields reference only RFDs in Accepted status or later.
The deferred numbering policy makes this natural: a draft has no number, so it
cannot appear in an `Extends` or `Extended by` field.

### 4. Related RFDs (Automated)

A `Related` metadata field is deliberately not introduced. At scale, maintaining
bidirectional `Related` links manually becomes a burden that outweighs the
benefit.

Instead, the VitePress site loader is extended to scan each RFD document for
links to other RFDs and surface the results automatically — both as a
"Referenced by" list on the individual RFD page and as data available to the
index. The `Date` column in the index is removed; the space is used for
relationship indicators instead.

## Drawbacks

- **Habit change.** Contributors are used to getting a number immediately.
  Drafts that circulate informally as "the thing I'm working on" can no longer
  be referred to by number. This is a feature, not a bug — but it requires
  adjustment.

- **`rfd-promote` is now a bigger operation.** It previously changed one line in
  one file. It now renames a file, updates a heading, creates a GitHub issue,
  and updates metadata. It must be atomic: if the GitHub issue creation fails,
  the file rename should not persist.

- **Tracking issues add noise to the GitHub issue tracker.** Mitigated by using
  a dedicated label (`rfd`) and ensuring the issue titles are clear
  (`RFD-041: ...`).

## Alternatives

### Assign numbers at Accepted, not Discussion

Numbers could be deferred further — only assigned when consensus is reached.
Rejected because Discussion is the point where external references become
necessary. Reviewers link to the RFD in comments; other authors need to
reference it in their own RFDs; tracking issues need a stable title. Discussion
is the right commitment point.

### `Related` as manual metadata

A `Related: RFD NNN, RFD MMM` field, maintained by authors, was considered.
Rejected because the maintenance cost grows with every new RFD written. A
site-level automation that scans document links is lower cost and covers the
same use cases without requiring authors to remember to update metadata in both
directions.

### Static tracking issue template

A fixed template for all tracking issues (boilerplate + link to RFD) was
considered. Rejected because the implementation phases differ significantly
between RFDs. A Design RFD with five phases needs five checkboxes; a Decision
RFD may need none. The LLM-assisted tool reads the Implementation Plan and
generates the task list accordingly.

## Non-Goals

- **Renumbering existing drafts.** Existing Draft-status RFDs at the time this
  ships keep their numbers under the grandfather clause.
- **Automated tracking issue updates.** The tracking issue is created once at
  Discussion promotion. Keeping it current (checking off tasks) is the author's
  and implementer's responsibility.
- **`Related` as manual metadata.** Covered by website automation.
- **Changes to the Discussion or Accepted lifecycle steps** beyond what is
  described above.

## Implementation Plan

### Phase 1: Deferred numbering tooling

Update `just rfd-draft` to create `NNN-title.md` with `NNN` as the literal
placeholder throughout (filename, heading, metadata). Update `just rfd-promote`
to detect when the current status is Draft (i.e., the file is named `NNN-*.md`),
assign the next available number, rename the file, and update the heading before
changing the status.

Update `docs/rfd/001-jp-rfd-process.md` to document the new policy.

Depends on: nothing. Can be merged independently.

### Phase 2: Tracking issue creation

Add `rfd_open_tracking_issue` to the `rfd` skill in `.jp/config/skill/rfd.toml`.
The tool accepts the RFD number, title, a URL to the document, and an array of
task strings derived from the Implementation Plan; it creates a GitHub issue via
`gh issue create` (or `curl` against the GitHub API) and returns the issue URL.

Extend `just rfd-promote` to call this tool when advancing to Discussion,
then inject the resulting issue number into the `Tracking Issue:` metadata
field. If issue creation fails, the promotion aborts before renaming the file.

Update `docs/rfd/001-jp-rfd-process.md` to document the tracking issue field
and its lifecycle (close on abandon/supersede).

Depends on: Phase 1 (the promotion command is extended, not replaced).

### Phase 3: `Extends` / `Extended by` metadata and `rfd-extend` command

Add `just rfd-extend NNN MMM` to the justfile. The command adds `Extended by:
RFD MMM` to `NNN`'s metadata and `Extends: RFD NNN` to `MMM`'s metadata.
Handle comma-separated lists: if the field already exists, append rather than
replace.

Update the metadata header documentation in `001-jp-rfd-process.md` and the
tooling table.

Depends on: nothing. Can be merged independently of Phases 1 and 2.

### Phase 4: Website automation for related RFDs

Extend the VitePress data loader (`docs/.vitepress/loaders/rfds.data.js`) to
parse each RFD document for Markdown links matching the pattern
`NNN-*.md` and record the set of referenced RFD numbers. Expose this as a
`references` field on each RFD data object. Compute `referenced_by` as the
inverse. Update the index page to use this data: remove the `Date` column,
add a references indicator, and surface `referenced_by` on individual RFD
pages.

Depends on: nothing. Can be merged independently.

## References

- [RFD 001: The JP RFD Process](001-jp-rfd-process.md) — the process this RFD
  extends.
- [RFD 003: JP-Assisted RFD Writing](003-jp-assisted-rfds.md) — the `rfd` skill
  that gains the `rfd_open_tracking_issue` tool.
