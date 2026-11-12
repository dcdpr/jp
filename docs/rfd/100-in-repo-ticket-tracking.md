# RFD 100: In-Repo Ticket Tracking

- **Status**: Implemented
- **Category**: Process
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-05
- **Extends**: [RFD 001], [RFD 041]
- **Extended by**: [RFD 102], [RFD 116], [RFD 127]
- **Summary**: Tracks work items as in-repo markdown tickets under
  `docs/ticket/` with a kanban board and RFD tracking integration.

## Summary

This document establishes **tickets**: lightweight work items tracked as
markdown files in the repository alongside RFDs.
Tickets support comments, sit on a kanban board, and can be imported from GitHub
issues, giving routine tasks and bugs a home outside the RFD process.

## Motivation

The RFD process is meant for significant architectural changes, but because the
repository has no lighter-weight format, RFDs have become the only place to
write anything down.
The corpus has grown to 98 published RFDs and 58 drafts, mixing brief task notes
with full system designs: RFD 068 is 585 words while RFD 065 is 4,638, and two
drafts share the same title.
Without a lighter option, every task climbs to an RFD.

GitHub Issues fill that role for external contributors, who should not have to
clone the repository to file a bug.
Within the repository workflow, however, GitHub Issues sit outside the context
that humans and AI assistants read day to day: assistants search the working
tree constantly and rarely query GitHub, issues do not appear in `git grep`, and
issue state is detached from the commits that resolve the work.

## What a Ticket Is

A ticket is a unit of work: a bug, a feature, or a chore.
It records what needs doing and the discussion around it.

|           | RFD                            | Ticket                         |
| --------- | ------------------------------ | ------------------------------ |
| Question  | What should we build, and why? | What needs doing?              |
| Contains  | A design and its rationale     | A description and a discussion |
| Lifetime  | Permanent record               | Closed when the work is done   |
| Deletable | No                             | Yes                            |

Write a ticket when the work is clear enough to start, and an RFD when it needs
a design first.
The deciding factor is whether there is an approach to debate: "the tool call
header misaligns below 80 columns" is a ticket even if the fix is subtle,
whereas "how should tool output be bounded?" is an RFD even if the
implementation is small.

If a ticket's discussion turns into a disagreement about design, promote the
ticket to an RFD draft.
This keeps design debates out of tickets without forcing routine task work into
RFDs.

Unlike RFDs, tickets can be deleted.
A ticket carrying false claims or imported spam is removed outright so tools and
assistants do not read it as valid context.
References to a deleted ticket render as dangling links.

> [!TIP]
> [RFD 102] replaces the counter with a time-ordered random id, so that two
> checkouts allocate without coordinating.
> Ids become collision-resistant rather than never-reused.

## Ticket Format

A ticket is a single markdown file at `docs/ticket/<id>-slug.md`.
The canonical reference form is `T-02wt0kx`; tooling also accepts `T02wt0kx`,
the bare ID (`02wt0kx`), and any letter case.
The ID's format and allocation are specified in [RFD 102].

The ticket ID appears only in the filename, not inside the document, so there is
no duplicate field to keep in sync and renaming a ticket is a plain file rename.

The description and full comment thread live in the same file, so reading a
ticket takes a single `fs_read_file` call, `cat` command, or web page load.

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

Tickets use the same `- **Key**: Value` metadata list as RFDs: it parses with a
three-line regex and renders directly as Markdown content.

| Field         | Required | Values                           |
| ------------- | -------- | -------------------------------- |
| `Status`      | yes      | `Todo`, `In Progress`, `Done`    |
| `Authors`     | yes      |                                  |
| `Date`        | yes      | `YYYY-MM-DD`                     |
| `Promoted to` | no       | The RFD this ticket became       |
| `GitHub`      | no       | `#123`, set by import            |
| `Label`       | no       | One `key=value` pair, repeatable |

Tickets do not store a close date in metadata.
The `ticket_*` tooling derives it from git history so the file never carries a
timestamp that can go stale.

#### Labels

A label is a `key=value` (or bare `key`) annotation classifying a ticket, such
as the kind of work (`type`) or the affected area (`client`, `package`).
Each board defines its own keys.
Unless a key's cardinality is constrained (see below), a ticket can carry any
number of labels, and a key may repeat: a ticket spanning two crates carries
both `package=jp_cli` and `package=jp_config`.

Each label is written on its own `- **Label**:` line rather than joined onto a
single line:

```markdown
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: package=jp_config
```

Writing one label per line keeps line length independent of the label count.
Every markdown file in the repository is formatted by `comfort`, which wraps
list items at 80 columns; a single line listing several labels would wrap onto
an indented continuation line that the metadata parser would read as the start
of the description.
`comfort` also escapes underscores that would otherwise parse as emphasis
(writing `package=jp\_cli` for `package=jp_cli`), and readers unescape them when
parsing.

Label syntax, key grammar, and on-disk invariants come from [`jp_label`], the
same `Labels` type used by conversation labels ([RFD 101]).
The ticket board declares its allowed label set through that crate's
`Vocabulary` type under `plugins.command.ticket.options.labels` in the workspace
configuration ([RFD 116]).
That setting is the sole source of truth for which labels exist, with one entry
per key:

```toml
[plugins.command.ticket.options.labels.<key>]
description = "<what the key covers>"
values = ["<value>"]
retired = ["<value>"]
min = 1
max = 1
unique = "key-value"
type = "date"
```

- **Allowed values (`values` and `retired`)**: Declaring a closed set prevents
  free-text near-synonyms that break grouping.
  `values = ["..."]` restricts the key to an explicit list.
  `values = []` declares an open key that accepts any value as written (for
  values that cannot be enumerated up front, such as an RFD number) while still
  supporting `min`, `max`, and `unique`.
  Omitting `values` declares a bare key (`<key>` with no `=value`); since a bare
  key is only present or absent, it accepts neither `min` nor `max`.
  If a board declares no vocabulary at all, any well-formed `key` or `key=value`
  label is accepted.

  A `retired` value remains valid on tickets that already carry it and is
  preserved when those tickets are edited, so relabelling an old ticket never
  forces its history to be rewritten, but the value cannot be added to new
  tickets.
  Removing a value from the vocabulary without retiring it causes `jp ticket
  check` and the docs build to fail on every ticket still carrying that value.

- **Cardinality (`min` and `max`)**: `min` and `max` bound how many values for a
  key a single ticket may carry.
  When omitted, a ticket may carry zero or more values.
  Setting `min = 1, max = 1` requires every ticket to carry the key exactly once
  (how JP's board requires `type` on every ticket); setting `max = 1` alone
  makes the key optional and non-repeatable.

- **Board-wide uniqueness (`unique`)**: `unique` restricts a key across all
  tickets on the board.
  `unique = "key"` allows at most one ticket on the board to carry the key
  regardless of value; `unique = "key-value"` allows at most one ticket per
  distinct `key=value` pair, so `rfd=116` and `rfd=117` can sit on different
  tickets while `rfd=116` can sit on only one.
  Combining `unique = "key"` with `min >= 1` is rejected because no two tickets
  could satisfy both.

- **Value type (`type`)**: `type` holds every value of an open key to one kind.
  `type = "date"` takes a calendar date written `YYYY-MM-DD`, zero-padded so
  that two dates order the same way as text, and refuses anything else, such as
  `1w` or `2026-1-5`.
  `date` is the only type.
  A key that lists its values, or a bare key, has no `type`.

- **Validation (`jp ticket check` and writes)**: Every label write validates the
  ticket against the vocabulary.
  Refused writes name the allowed values to choose from or the existing ticket
  that already holds a `unique` label.
  Reading is permissive: a ticket always lists the labels written in its file,
  and undeclared labels are reported by validation rather than hidden.
  `jp ticket check` (and the docs build) checks every ticket against all write
  rules: unparseable ticket files, undeclared keys or values, values that aren't
  of their key's `type`, `min` and `max` violations, duplicate `unique` labels
  across tickets (such as after a hand edit or merge), duplicate ticket IDs
  across files, and legacy four-digit filenames from before [RFD 102] that
  commands ignore.
  `jp ticket check` exits with status 1 on any violation so it can run in CI.

List filtering accepts either a `key=value` pair or a bare `key`: `--label
package=jp_cli` matches that exact value, while `--label package` matches any
value under `package`.
Repeating `--label` narrows results further, requiring every specified filter to
match.
A date is also compared with `<`, `<=`, `>`, or `>=`: `--label 'due<2026-12-01'`
matches every ticket whose `due` is a date before the 1st.
The operand has to be a date, and a value under the key that isn't one never
matches.
A filter reads labels as written rather than through the vocabulary, so a ticket
carrying a malformed value is still found by its key.

Label subcommands add and remove individual labels rather than overwriting the
full list, so a write never drops unrelated labels:

```sh
jp ticket label add --id T-02wt0kx client=cli   # keeps what the ticket carries
jp ticket label set --id T-02wt0kx type=bug      # replaces what `type` holds
jp ticket label rm --id T-02wt0kx package        # takes every `package` label
```

`--id` can appear before or after the subcommand and accepts repeated flags or
comma-separated IDs to update multiple tickets atomically.
Adding a label the ticket already carries, or removing one it lacks, changes
nothing and reports that no change was made, making retries idempotent.
`jp ticket edit --label` is the only write that replaces the entire label list,
and it reports any labels it removed.
Because validation checks the existing ticket as well as the vocabulary, a
retired value already on the ticket remains valid across all of these commands.

The `ticket_*` assistant tools expose valid label tokens as a JSON Schema `enum`
so the assistant selects from the allowed set instead of guessing.
Because an `enum` cannot enumerate every value of an open key (`values = []`), a
board with an open key omits `enum` from the schema and describes its keys in
prose instead; invalid guesses are still rejected on write with the list of
valid labels.
`ticket_create` and `ticket_edit`'s `add_labels` include only active values,
while `ticket_list` also includes retired values so the assistant can filter for
tickets that still carry them.
`ticket_edit`'s `remove_labels` removes specific `key=value` pairs, or every
label under a key when given a bare `key`.

Because tool declarations cannot read the workspace vocabulary directly, `jp
ticket tools sync` mirrors the allowed tokens into a generated file alongside
them (`.jp/mcp/tools/ticket/synced.toml` in JP).
This mirror is a schema hint rather than an enforcement contract: writes always
validate against the vocabulary, so a stale mirror degrades to a corrective
round trip rather than an invalid write, and editing the vocabulary does not
break tests.
`synced.toml` is a stopgap until plugins can describe the tools they serve ([RFD
116]).

GitHub import never sets labels.
The repository owns the metadata block, so imported issues are labelled during
triage.

### Comments

Comments are separated by a line of five or more dashes (`-----`) and open with
their own metadata block.
`From` is a short handle (`jean`, `jp`), or `gh:username` for imported GitHub
comments.

The structure of the file is append-only: comment blocks are never removed or
reordered, and adding a comment appends to the end of the file.
Block contents remain editable, as on GitHub, so descriptions can grow as
information arrives and authors can edit their own comments.
Deleting a comment preserves the block and replaces its body with a marker
naming the reason (`deleted`, `off-topic`, or `spam`).

Replies are recorded as `- **Re**: #1`, referencing a comment on the same ticket
by its 1-based position.
Storage stays flat while the website and terminal render the thread.
Because positions are not stable identifiers, concurrent appends on two branches
can shift comment indices and leave a reply pointing at the wrong comment.
That tradeoff is accepted for now; stable comment IDs are the fix if concurrent
replies become a problem.

### Checklists

A description can include Markdown task-list items to track the steps or
acceptance criteria of the work:

```markdown
- [x] Keep tools declared after a result
- [ ] Cover a second tool call
```

A checklist item is a line opening with optional indentation, a list marker,
whitespace, a checkbox (`[ ]` or `[x]`), whitespace, and item text.
Task-list lines inside fenced code blocks or comments (which tooling only
appends) are ignored.

Checklist progress is counted from the description on read and is never written
into metadata, so checkboxes ticked by hand and by tooling cannot disagree.
Tooling ticks an item by matching its text and changing only the character
inside its checkbox.
Closing a ticket with unchecked items is allowed and reported rather than
refused: the checklist belongs to the ticket, and whoever closes it decides
whether unchecked items remain relevant.

A checklist item represents a step within one piece of work.
Work that can be picked up, merged, or debated on its own belongs in a separate
ticket.

### Parsing

A comment boundary is a line of five or more dashes at column zero, followed by
a blank line and a metadata block containing both `From` and `Date`.
Content inside fenced code blocks is skipped when scanning for boundaries.
Everything before the first boundary is the description.

The `## Comments` heading is decorative: tooling inserts it before the first
comment and never consults it when parsing, so a heading of that name inside a
description or comment body is harmless.

## The Board

Tickets sit on a kanban board with three columns:

```
Todo  →  In Progress  →  Done
```

Columns represent stages of work, and vertical order within a column represents
priority.
`Todo` is the prioritized work queue, read top-down, and holds triaged and
untriaged work alike, including newly imported bug reports awaiting triage.
`Done` is ordered newest-first; the board view shows only the head of the
column, while full history lives in the ticket index.

Blocking is not part of the ticket format, and the board has no dedicated column
or badge for it.
Projects that mark blocked tickets use a label, which the board renders like any
other label.

Board state is split across two places, matching the RFD priority board:

- **Status** lives in the ticket file, since workflow status is semantic and
  belongs to the ticket.
- **Order within a column** lives in a single board file, since rank is
  relational across tickets.
  Encoding rank as a field on each ticket would mean reordering a column
  rewrites a dozen ticket files.

## Relationship to RFDs

Tickets and RFDs are separate document kinds with separate numbering and
lifecycles.
They connect in three places.

### RFD Tracking Tickets

When an RFD is accepted, `jp rfd promote` offers to file its **tracking ticket**
(replacing the GitHub tracking issue from [RFD 041] §2): one ticket labelled
`tracking-rfd=045`, with the phases of the Implementation Plan as its
description and one checklist item per phase.
Filing is an interactive prompt rather than an automatic step, matching the
earlier tracking-issue prompt, because acceptance records an agreed design
direction rather than a commitment to start building immediately.
Whoever accepts the prompt reviews the ticket before it is created.

If declined at acceptance, `jp rfd track` files the tracking ticket later, and
`jp rfd start` files it when work starts if none exists.
Both commands are idempotent and reuse an existing tracking ticket rather than
duplicating it.

A phase that needs its own card is filed separately with `relevant-rfd=045`, as
is any other ticket whose work belongs to the RFD.
Splitting phases into separate tickets happens when the work is picked up, not
at acceptance when the phases are still a plan.

The two label keys are configured on `jp rfd` via `tracking.key` and
`relevant.key` ([RFD 116]), so the ticket format itself has no built-in RFD
concepts.
A board that defines a label vocabulary declares both keys as open (`values =
[]`), with `max = 1` and `unique = "key-value"` on the tracking key so each
tracking ticket maps one-to-one with an RFD.

### Promoting Tickets to RFDs

When a ticket's discussion turns into a design question, promoting the ticket
seeds an RFD draft from it and closes the ticket as `Done` with `Promoted to`
pointing at the draft.

### RFD Priority Board and `inDevelopment`

The `inDevelopment` flag is removed from `docs/rfd/priority.json`.
Whether someone is actively writing code is a property of a work item rather
than a design document; the flag lived on the RFD board only because tickets did
not exist.

Instead, `inDevelopment` is derived from the RFD's `tracking-rfd=NNN` ticket: an
RFD is in development while its tracking ticket sits in the `In Progress`
column.
Phase tickets (`relevant-rfd=NNN`) do not set the flag.

Work starts and stops on the ticket board by moving the tracking ticket.
The RFD board displays the derived flag and links each RFD that has tickets to
the ticket board, filtered to tickets carrying either `tracking-rfd=NNN` or
`relevant-rfd=NNN`.
No status history is written to the ticket file, since git history already
timestamps every change.

RFD priority ordering stays in `docs/rfd/priority.json` because the two boards
rank different queues:

|          | RFD priority board          | Ticket board              |
| -------- | --------------------------- | ------------------------- |
| Question | What should we design next? | What are we building now? |
| Holds    | Ranked design pipeline      | Work items that exist     |

Most of the RFD backlog consists of design ideas rather than committed work
items, so creating a ticket for every RFD would flood the ticket board.

## GitHub Issues

GitHub Issues remain the front door for outside contributors and are imported as
tickets one-way.

Each import replaces the ticket's title, description, and comments wholesale
while leaving the metadata block untouched.
GitHub owns the issue prose and comments, while the repository owns `Status` and
`Label` metadata, so an imported ticket can be triaged and moved across the
board without the next import overwriting its state.

Imported tickets are read-only for discussion locally: replies go on GitHub and
arrive on the next import, so there is no divergence to reconcile and nothing is
written back to GitHub.

Imported text is treated as untrusted input.
Because the documentation site compiles markdown as Vue, imported bodies are
escaped before they reach the working tree and render as data, never as
executable page source.

## Alternatives

**Git objects instead of files.** [PR #872] proposes storing issues as
per-writer, append-only operation logs under a dedicated ref namespace, folded
deterministically so concurrent edits never conflict.
While that data model avoids merge conflicts across offline replicas, custom
refs are unreachable by `fs_read_file` and `git grep`, invisible to the site
build, and absent from pull-request diffs, which are the three properties this
design exists to provide.
Storing tickets as files accepts merge conflicts on the board file and on
concurrent comment appends in exchange for standard repository visibility.
[PR #872] also stores priority per issue (so re-ranking rewrites one object per
issue rather than a single board file) and tombstones issues rather than
deleting them.

## Non-Goals

- **Assignees and milestones.** Deferred until needed.
  Labels were added once grouping by area started to matter; see
  [Labels](#labels).
- **Label hierarchies, colours, or per-label board columns.** A label is only a
  key-value pair and a description.
- **A search DSL.** Use `jp ticket list --query`, or `rg` over the markdown
  files.
- **Write-back to GitHub.** GitHub import is one-way.
- **Notifications.** Contributors check the board directly.
- **Stable comment ids.** 1-based positions suffice unless concurrent replies
  become a problem in practice.
- **Replacing RFDs.** Tickets lower the bar for tracking work items; they do not
  replace RFDs for design decisions.

## Risks and Open Questions

- **Two boards may be one too many.** If the RFD board goes stale once tickets
  exist, fold RFDs into the ticket board as cards and retire `priority.json`.

- **Ticket sprawl replaces RFD sprawl.** Unlike RFDs, tickets can be deleted and
  closed tickets leave the active board view, which bounds active clutter.

- **Agent comment volume.** A ticket with forty comments is a large file to
  read.
  The mitigation is keeping tickets scoped and closing them sooner rather than
  splitting a ticket across multiple files.

## Implementation Plan

This document specifies the file format, vocabulary, board semantics, and RFD
integration.
Implementation proceeds in six phases:

1. **Format and lifecycle.** Create `docs/ticket/`, implement the comment
   parser, and add commands to create, comment on, close, and list tickets.
2. **Assistant tools.** Expose `ticket_*` tools through `.jp/mcp/tools/`.
3. **Website and board.** Build a `/ticket/` index and kanban board, retire the
   `inDevelopment` field from `priority.json`, and derive it from the tracking
   ticket (moved by the RFD board's checkbox).
4. **GitHub import.** Implement one-way import for GitHub issues and comments.
5. **RFD seams.** Add the tracking-ticket prompt and idempotent ticket-to-RFD
   promotion.
   Amend [RFD 001] and [RFD 041] (including fixing 041's stale claim that
   tracking issues are created at Discussion), and add a TIP to [RFD 041] once
   this RFD holds a permanent number.
6. **Shared substrate.** Extract shared metadata parsing, ID allocation, and
   board state once both implementations have settled.

[PR #872]: https://github.com/dcdpr/jp/pull/872
[RFD 001]: 001-jp-rfd-process.md
[RFD 041]: 041-rfd-lifecycle-enhancements.md
[RFD 101]: 101-conversation-labels.md
[RFD 102]: 102-collision-resistant-ticket-identifiers.md
[RFD 116]: 116-shareable-rfd-ticket-and-site-tooling.md
[RFD 127]: 127-in-repo-project-wiki.md
[`jp_label`]: https://github.com/dcdpr/jp/tree/main/crates/jp_label
