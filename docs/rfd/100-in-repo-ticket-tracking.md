# RFD 100: In-Repo Ticket Tracking

- **Status**: Implemented
- **Category**: Process
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-05
- **Extends**: [RFD 001], [RFD 041]
- **Extended by**: [RFD 102], [RFD 116]
- **Summary**: Lightweight markdown-based work items tracked in-repo, displacing
  GitHub issues from assistant context with kanban board and RFD promotion
  mechanics.

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
References to a deleted ticket render as dangling.

> [!TIP]
> [RFD 102] replaces the counter with a time-ordered random id, so that two
> checkouts allocate without coordinating.
> Ids become collision-resistant rather than never-reused.

## Ticket Format

A ticket is a single markdown file at `docs/ticket/<id>-slug.md`.
The canonical reference form is `T-02wt0kx`; tooling accepts `T02wt0kx`, the
bare body, and any case.
The id's shape and allocation are specified in [RFD 102].

The filename carries the id and the document does not, so there is one place
that names a ticket and nothing to keep in step.
Renaming a ticket is a rename.

Description and full discussion live in that one file, so reading a ticket is
one `fs_read_file`, one `cat`, or one page on the website.

````markdown
# Tool call header misaligned

- **Status**: In Progress
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-05
- **Label**: client=cli
- **Label**: package=jp_printer
- **Label**: relevant-rfd=095
- **Label**: type=bug

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
- **Re**: #1

The wrap calculation in `jp_printer` uses the pre-indent width:

```rust
let available = width - indent;
```

-----
````

### Metadata

Tickets use the same `- **Key**: Value` idiom as RFDs, for the same reason: it
parses with a three-line regex and renders as visible content.

| Field         | Required | Values                           |
| ------------- | -------- | -------------------------------- |
| `Status`      | yes      | `Todo`, `In Progress`, `Done`    |
| `Authors`     | yes      |                                  |
| `Date`        | yes      | `YYYY-MM-DD`                     |
| `Promoted to` | no       | The RFD this ticket became       |
| `GitHub`      | no       | `#123`, set by import            |
| `Label`       | no       | One `key=value` pair, repeatable |

There is no close date.
The `ticket_*` tooling reads it from git history, so the file never carries a
timestamp that can go stale.

#### Labels

A label is a `key=value` annotation classifying a ticket: what type of work it
is, and what part of the system it lands in.
JP's board says the first with `type` and the second with keys such as `client`
and `package`; another board picks its own.
A ticket carries any number of them, or none, unless the board bounds a key (see
below), and a key may repeat: a ticket spanning two crates carries
`package=jp_cli` and `package=jp_config`.

The field is written once per pair rather than joining them onto one line:

```markdown
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: package=jp_config
```

That is not cosmetic.
Every markdown file in the repository is formatted by `comfort`, which wraps a
list item at 80 columns; a single line listing several long values would be
wrapped onto an indented continuation that the metadata parser reads as the
start of the description.
One pair per line means no line grows with the number of labels.
The formatter also escapes what would otherwise be emphasis, so `package=jp_cli`
is written `package=jp\_cli` and every reader undoes that.

The shape, the key grammar, and the on-disk invariants come from [`jp_label`],
the same `Labels` conversation labels use ([RFD 101]); the vocabulary below is
the ticket board's own, declared through that crate's `Vocabulary`.

The vocabulary lives in the workspace configuration, under
`plugins.command.ticket.options.labels` ([RFD 116]).
That setting is the only source of truth for which labels exist; nothing here
enumerates them.
It declares one entry per key, with the values that key accepts:

```toml
[plugins.command.ticket.options.labels.<key>]
description = "<what the key covers>"
values = ["<value>"]
retired = ["<value>"]
min = 1
max = 1
unique = "key-value"
```

`min` and `max` bound how many of the key's values one ticket carries.
Left out, a ticket carries any number of them, or none.
`min = 1, max = 1` makes a key every ticket carries exactly once, which is how
JP's board makes `type` say what kind of work every ticket is; `max = 1` alone
makes one optional, but never repeated.
Every label write is held to them, and the refusal names the values to choose
from; the docs build reports a ticket that breaks them, such as one edited by
hand.
A key that leaves `values` out is bare: present or absent, so it takes neither.

`unique` holds a key to one ticket across the board.
`unique = "key"` lets one ticket carry the key at all, whatever its value;
`unique = "key-value"` lets one ticket carry each of its values, so `rfd=116`
and `rfd=117` sit on two tickets but `rfd=116` on only one.
Every label write checks it against the other tickets, and the refusal names the
ticket already carrying the label; the docs build reports two tickets carrying
one, such as after a hand edit or a merge.
`unique = "key"` on a key every ticket needs is refused, since no two tickets
could meet it.

A closed set is the point of declaring one.
Free-text labels accumulate near-synonyms and grouping stops working.
A write naming a key or value the vocabulary doesn't declare is refused, and the
refusal names the ones that can be added.
A key declared with `values = []` is open: it takes any value, for labels whose
values can't be listed up front, such as an RFD number.
Its values are spelled as given, and it can be bounded and made unique like any
other key.
A board that declares no vocabulary has no set to hold a label to, so it takes
any well-formed `key` or `key=value` label as written.

**Retiring is not deleting.** A retired value stays valid on a ticket that
already carries it and can be listed again on a write to that ticket, so
relabelling an old ticket never forces its history to be rewritten.
It cannot be added anywhere new.
Deleting the entry outright is the other option, and it turns every ticket
carrying that value into a docs-build failure until they are fixed.

Reading is liberal throughout: a ticket lists the labels it carries as written,
and the docs build reports one the vocabulary doesn't declare rather than hiding
it.

Filtering takes a pair or a bare key: `--label package=jp_cli` matches that
value, `--label package` matches any value under the key.
Repeating the flag narrows further, so each term is an additional requirement.

Labelling adds and removes rather than replacing, so a write never drops a label
it doesn't name:

```sh
jp ticket label add --id T-02wt0kx client=cli   # keeps what the ticket carries
jp ticket label set --id T-02wt0kx type=bug      # replaces what `type` holds
jp ticket label rm --id T-02wt0kx package        # takes every `package` label
```

`--id` names the tickets, on either side of the verb; repeat it, or separate ids
with commas, to change several at once, all of them or none.
Adding a label the ticket carries, or removing one it lacks, changes nothing and
says so, so a retried call lands the same way twice.
`jp ticket edit --label` is the one write that sets the whole list, and it names
the labels it removed.
Because the check is against the ticket as well as the vocabulary, a retired
value the ticket already has stays through any of them.

The `ticket_*` tools advertise the accepted tokens as a JSON Schema `enum`, so
the assistant is offered the set rather than guessing at it.
An `enum` can't hold every value of an open key, so a board with one gets no
`enum`: its labels are described in words instead, and a wrong guess is refused
with the labels it can use.
`ticket_create` and `ticket_edit`'s `add_labels` are offered only what they may
add; `ticket_list` also gets retired values, because finding what still carries
one means filtering on it.
`ticket_edit` takes labels off with `remove_labels`, where a bare key takes
every label under it.
The tool declarations cannot read the vocabulary themselves, so `jp ticket tools
sync` mirrors the tokens into a file it generates next to them,
`.jp/mcp/tools/ticket/synced.toml` in JP.
The mirror is a hint rather than a contract: writes are validated against the
vocabulary, so a stale mirror degrades to a corrective round trip and never to a
wrong write, and editing the vocabulary never fails a test.
It is a stopgap, and goes once a plugin can describe the tools it serves ([RFD
116]).

An import never sets labels.
The repository owns the metadata block, so an imported issue is labelled here by
whoever triages it.

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

Replies are recorded as `- **Re**: #1`, referencing a comment by its 1-based
position.
A reply always targets a comment on the same ticket, so it names no id.
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

Blocking is not part of the format, and the board has no column or badge for it:
a project that marks blocked tickets does it with a label, which the board shows
like any other.

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

### An accepted RFD gets a tracking ticket, optionally

When an RFD is accepted, `jp rfd promote` offers to file its tracking ticket:
one ticket labelled `tracking-rfd=045`, with the phases of the Implementation
Plan as its description.
A prompt, not an automatic step — the same shape as the existing tracking-issue
prompt — because acceptance records an agreed direction, not a commitment to
start building.
Whoever accepts the prompt reviews the ticket before it lands.

`jp rfd track` files it later, and `jp rfd start` files it when the work starts
if it has none.
Both are idempotent: an existing tracking ticket is reused, never duplicated.

A phase that needs its own card is filed separately, labelled
`relevant-rfd=045`, as is any other ticket whose work belongs to the RFD.
The fan-out is a decision made when the work is picked up, not at acceptance,
where the phases are still a plan.

The two keys are settings of `jp rfd`, `tracking.key` and `relevant.key` ([RFD
116]), so the ticket format knows nothing about RFDs.
A board that defines labels defines both keys, open to any value; `max = 1` and
`unique = "key-value"` on the tracking key hold a ticket to tracking one RFD and
an RFD to one tracking ticket.

This replaces the GitHub tracking issue from [RFD 041] §2.

### Tickets promote to RFDs

A ticket whose discussion turns into a design question is promoted: the tooling
seeds an RFD draft from the ticket, and the ticket closes as `Done` with
`Promoted to` pointing at the draft.
The work item is finished; the work moved.

### The RFD board keeps ordering; `inDevelopment` moves to the ticket

The `inDevelopment` flag leaves `docs/rfd/priority.json`.
"Someone is currently writing code for this" is a property of a work item, not
of a design document — it was on the RFD board only because tickets did not
exist.

Each RFD gets at most one **tracking ticket**: the ticket labelled
`tracking-rfd=NNN`, holding the implementation plan as its description.
It plays the role a tracking issue plays elsewhere; a phase that needs its own
card gets a separate ticket, labelled `relevant-rfd=NNN`.

The flag becomes **derived** from that one ticket: an RFD is in development when
its tracking ticket sits in the In Progress column.
Phase tickets never light the flag.

Work starts and stops on the ticket board, by moving the tracking ticket.
The RFD board shows the flag and links each RFD with tickets to the ticket
board, narrowed to the tickets labelled with either key for that RFD.
No status history is written to the ticket — the file's git history already
dates every change.

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
GitHub owns what was written on GitHub; the repository owns `Status` and the
labels, so an imported ticket can be triaged and moved across the board without
the next import undoing it.

Imported tickets are read-only for discussion: replies go on GitHub and arrive
on the next import, so there is no divergence to reconcile and nothing is ever
written back.

Imported text is untrusted input.
The site compiles markdown as Vue, so imported bodies are escaped before they
reach the working tree and are rendered as data, never as page source.

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

- **Assignees and milestones.** Add them when the absence hurts.
  Labels landed later, once grouping by area started to matter; see
  [Labels](#labels).
- **Label hierarchies, colours, or per-label board columns.** A label is a name
  and a description.
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
   Retire the `inDevelopment` field and derive it from the tracking ticket,
   which the RFD board's checkbox moves.
4. **GitHub import.** One-way, issues and comments.
5. **RFD seams.** The tracking-ticket prompt and ticket-to-RFD promotion, which
   needs an idempotent retry story.
   Amend [RFD 001] and [RFD 041], including 041's stale claim that tracking
   issues are created at Discussion, and add a TIP to 041 once this RFD holds a
   permanent number.
6. **Shared substrate.** Extract the duplicated metadata parsing, id allocation,
   and board state — if the duplication has proven itself.

[PR #872]: https://github.com/dcdpr/jp/pull/872
[RFD 001]: 001-jp-rfd-process.md
[RFD 041]: 041-rfd-lifecycle-enhancements.md
[RFD 101]: 101-conversation-labels.md
[RFD 102]: 102-collision-resistant-ticket-identifiers.md
[RFD 116]: 116-shareable-rfd-ticket-and-site-tooling.md
[`jp_label`]: https://github.com/dcdpr/jp/tree/main/crates/jp_label
