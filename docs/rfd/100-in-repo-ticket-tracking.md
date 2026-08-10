# RFD 100: In-Repo Ticket Tracking

- **Status**: Implemented
- **Category**: Process
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-05
- **Extends**: [RFD 001], [RFD 041]

## Summary

This document establishes **tickets**: lightweight work items tracked as
markdown files in the repository, alongside RFDs.
Tickets carry comments, sit on a kanban board, and can be imported from GitHub
issues.
They take over the work that RFDs have been absorbing but were never meant to
hold.

## Motivation

The RFD process works for what it was designed for.
It has also become the only place to write anything down, and the corpus shows
it: 98 published RFDs and 58 drafts, with one-pagers sitting next to real
designs.
RFD 068 is 585 words.
RFD 065 is 4638.
Two drafts share a title.

When "significant architectural change" is the bar and there is no lower rung,
everything climbs to the top rung.

GitHub Issues are the obvious lower rung, and they stay — outside contributors
should not have to clone a repository to file a bug.
But assistants grep the repository constantly and rarely search GitHub, so
issues sit outside the context that actually gets read.
They are also absent from `git grep` and detached from the commits that resolve
them.

## What a Ticket Is

A ticket is a unit of work: a bug, a feature, a chore.
It records what needs doing and the discussion around it.

|           | RFD                            | Ticket                         |
| --------- | ------------------------------ | ------------------------------ |
| Question  | What should we build, and why? | What needs doing?              |
| Contains  | A design and its rationale     | A description and a discussion |
| Lifetime  | Permanent record               | Closed when the work is done   |
| Deletable | No                             | Yes                            |

Write a ticket when the work is clear enough to start, an RFD when it needs a
design first.
The signal is whether there is a decision to argue about: "the tool call header
misaligns below 80 columns" is a ticket even if the fix is subtle; "how should
tool output be bounded?" is an RFD even if the implementation is trivial.

A ticket whose discussion turns into a disagreement about approach is promoted
to an RFD draft — the pressure valve that keeps design work out of tickets
without pushing task work back into RFDs.

Unlike RFDs, tickets can be deleted — a ticket carrying false claims or
imported spam is removed outright, so that nothing reads it as true.
Ticket numbers are never reused: the next id comes from a monotonic counter, not
from the highest file on disk.
References to a deleted ticket render as dangling.

## Ticket Format

A ticket is a single markdown file at `docs/ticket/NNNN-slug.md`, where `NNNN`
is a zero-padded sequential number.
The canonical reference form is `T0042`; tooling accepts `42`, `042`, and `T42`.

Description and full discussion live in that one file, so reading a ticket is
one `fs_read_file`, one `cat`, or one page on the website.

````markdown
# T0042: Tool call header misaligned

- **Status**: In Progress
- **Kind**: Bug
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-05
- **Implements**: 095

The header renders one column left of the body when `style.parameters` is
`function_call` and the terminal is narrower than 80 columns.

## Comments

-----

- **From**: jean
- **Date**: 2026-08-05T14:03:11Z

Reproduced at 72 columns. Not at 80.

-----

- **From**: jp
- **Date**: 2026-08-05T14:31:02Z
- **Re**: T0042#1

The wrap calculation in `jp_printer` uses the pre-indent width:

```rust
let available = width - indent;
```

-----
````

### Metadata

Tickets use the same `- **Key**: Value` idiom as RFDs, for the same reason: it
parses with a three-line regex and renders as visible content.

| Field         | Required | Values                         |
| ------------- | -------- | ------------------------------ |
| `Status`      | yes      | `Todo`, `In Progress`, `Done`  |
| `Kind`        | yes      | `Bug`, `Feature`, `Chore`      |
| `Authors`     | yes      |                                |
| `Date`        | yes      | `YYYY-MM-DD`                   |
| `Blocked by`  | no       | `T0041`, or free text          |
| `Implements`  | no       | The RFD this ticket implements |
| `Promoted to` | no       | The RFD this ticket became     |
| `GitHub`      | no       | `#123`, set by import          |
| `Source`      | no       | `scheme:id`, set by import     |

`GitHub` and `Source` are import links: each carries the value that identifies
the ticket's origin, so a second import of the same item finds the ticket it
already wrote rather than filing another one.
A ticket carries at most one of them.

`Source` is free-form provenance, for anywhere that isn't GitHub.
The scheme names the place and the id identifies the item there; the repository
never interprets either, so whatever wrote the ticket is the only thing that has
to recognise its own scheme.
A marker is the one piece of imported text written without escaping, since it
has to match byte for byte on the way back out, so ids are restricted to
single-line text and refused rather than mangled when they aren't.

There is no close date.
The `ticket_*` tooling reads it from git history, so the file never carries a
timestamp that can go stale.

### Comments

Comments are separated by a line of five or more dashes and open with their own
metadata block.
`From` is a short handle (`jean`, `jp`); imported GitHub comments use
`gh:username`.

The *structure* of the file is append-only: comment blocks are never removed or
reordered, and a new comment is a pure append at EOF.
Block *contents* are freely editable, as on GitHub — descriptions grow as
information arrives, and authors fix their own comments.
Deleting a comment replaces its body with a marker naming the reason (`deleted`,
`off-topic`, `spam`) and keeps the block.

Replies are recorded as `- **Re**: T0042#1`, referencing a comment by its
1-based position.
Storage stays flat; the website and terminal render the thread.
Positions are not identifiers: two branches appending concurrently can shift
one, and a reply then points at the wrong comment.
Accepted rather than solved; stable ids are the fix if it ever bites.

### Parsing

A comment boundary is a line of five or more dashes at column zero, followed by
a blank line, followed by a metadata block containing both `From` and `Date`.
Content inside fenced code blocks is skipped.
Everything before the first boundary is the description.

The `## Comments` heading is decorative.
Tooling inserts it before the first comment and never consults it when parsing,
so a heading of that name anywhere in a description or comment body is harmless.

## The Board

Tickets sit on a kanban board with three columns:

```
Todo  →  In Progress  →  Done
```

Columns are stages of work; priority is the vertical order within a column.
Todo is therefore the prioritized work queue, read top-down, and it holds
triaged and untriaged work alike — an imported bug report that may not be a bug
still belongs there.

`Blocked` is a metadata field rendered as a badge, not a column: a blocked
ticket is still at whatever stage it reached, and a column would lose that.

Done is ordered like any other column, newest first, and the board view shows
only the head of it.
Full history lives in the ticket index.

Two pieces of state, kept apart:

- **Status** lives in the ticket file.
  It is semantic and belongs to the ticket.
- **Order within a column** lives in a single board file.
  It is relational and belongs to no individual ticket.

Encoding rank as a ticket field would mean every drag rewrites a dozen files.
Same split the RFD priority board already uses.

## Relationship to RFDs

Tickets and RFDs are separate document kinds with separate numbering and
separate lifecycles.
They meet in three places.

### RFD phases become tickets, optionally

When an RFD is accepted, `rfd-promote` offers to turn each phase of its
Implementation Plan into a ticket carrying `Implements: 045`.
A prompt, not an automatic step — the same shape as the existing tracking-issue
prompt — because acceptance records an agreed direction, not a commitment to
start building.
Whoever accepts the prompt reviews the resulting tickets before they land.

This replaces the GitHub tracking issue from [RFD 041] §2, and one ticket now
serves as both the tracking item and the prioritized work item.

### Tickets promote to RFDs

A ticket whose discussion turns into a design question is promoted: the tooling
seeds an RFD draft from the ticket, and the ticket closes as `Done` with
`Promoted to` pointing at the draft.
The work item is finished; the work moved.

### The RFD board keeps ordering; it loses `inDevelopment`

The `inDevelopment` flag leaves `docs/rfd/priority.json`.
"Someone is currently writing code for this" is a property of a work item, not
of a design document — it was on the RFD board only because tickets did not
exist.
It becomes **derived**: an RFD is in development when any ticket carrying
`Implements: NNN` sits in the In Progress column.
Derived, not synced, so there is nothing to keep in step.

RFD priority ordering stays.
The two boards rank different things:

|          | RFD priority board          | Ticket board              |
| -------- | --------------------------- | ------------------------- |
| Question | What should we design next? | What are we building now? |
| Holds    | Ranked design pipeline      | Work items that exist     |

Most of the RFD backlog is not a work item — nobody has said "this needs doing"
about it beyond "we should think about it" — so a ticket per RFD would flood
the board.

## GitHub Issues

GitHub Issues remain the front door for outside contributors.
They are imported as tickets, one way only.

Each import replaces the ticket's content — title, description, comments —
wholesale, and never touches the metadata block.
GitHub owns what was written on GitHub; the repository owns `Status`, `Kind`,
`Blocked by`, and `Implements`, so an imported ticket can be triaged and moved
across the board without the next import undoing it.

Imported tickets are read-only for discussion: replies go on GitHub and arrive
on the next import, so there is no divergence to reconcile and nothing is ever
written back.

Imported text is untrusted input.
The site compiles markdown as Vue, so imported bodies are escaped before they
reach the working tree and are rendered as data, never as page source.

## Other Sources

A thought caught in a note-taking app is a thought that does not reach the
board, and everyone's capture habit is their own.
Rather than name those places, `jp ticket add --source scheme:id` records where
a ticket came from and refuses to file the same item twice, which is enough for
someone to write the sweep they want outside the repository.

Ownership moves, which is the difference from GitHub.
An item is captured once and the ticket is where the work happens from then on,
so a second add naming the same source leaves the ticket alone: the write-up
done here is never overwritten from a scratch pad.

That also makes the repository the cursor.
A sweep reads the sources already on the board to know what it has seen, so it
keeps no state of its own and stays correct across a machine it has not run on
in a week.

## Alternatives

**Git objects instead of files.** [PR #872] proposes issues as per-writer,
append-only operation logs under a dedicated ref namespace, folded
deterministically so concurrent edits never conflict.
It is the stronger data model and the wrong fit here: refs are unreachable by
`fs_read_file` and `git grep`, invisible to the site build, and absent from
pull-request diffs — the three properties this design exists to provide.
Its conflict-freedom solves offline editing across replicas that cannot see each
other, which is not this project.
The cost accepted instead is merge conflicts on the board file and on concurrent
appends.

It also keeps priority in the store, where a re-rank rewrites one object per
issue rather than one board file, and tombstones rather than deletes.

## Non-Goals

- **Labels, assignees, milestones.** Add them when the absence hurts.
- **A search DSL.** `just ticket-grep` over markdown files.
- **Write-back to GitHub.**
- **Notifications.** The board is the notification.
- **Stable comment ids.** Positions are good enough until they are not.
- **Replacing RFDs.** This lowers the bar for entry; it does not remove the top
  rung.

## Risks and Open Questions

- **Two boards may be one too many.** If the RFD board goes stale once tickets
  exist, fold RFDs into the ticket board as cards and retire `priority.json`.

- **Ticket sprawl replaces RFD sprawl.** Mitigated only by tickets being
  deletable and closed tickets leaving the board — neither true of RFDs.

- **Agent comment volume.** A ticket with forty comments is a large file to
  read.
  The answer is closing tickets sooner, not splitting the file.

## Implementation Plan

This document fixes the file format, the vocabulary, the board semantics, and
the relationship to RFDs.
How the tooling gets there is not fixed.

1. **Format and lifecycle.** `docs/ticket/`, the comment parser, and recipes to
   create, comment, close, and list.
2. **Assistant tools.** `ticket_*` tools through `.jp/mcp/tools/`.
   The point of the exercise.
3. **Website and board.** A `/ticket/` index and the kanban board.
   Retire `inDevelopment` and derive it from ticket state.
4. **GitHub import.** One-way, issues and comments.
5. **RFD seams.** The phase-ticket prompt and ticket-to-RFD promotion, which
   needs an idempotent retry story.
   Amend [RFD 001] and [RFD 041], including 041's stale claim that tracking
   issues are created at Discussion, and add a TIP to 041 once this RFD holds a
   permanent number.
6. **Shared substrate.** Extract the duplicated metadata parsing, id allocation,
   and board state — if the duplication has proven itself.

[PR #872]: https://github.com/dcdpr/jp/pull/872
[RFD 001]: 001-jp-rfd-process.md
[RFD 041]: 041-rfd-lifecycle-enhancements.md
