# RFD 001: The JP RFD Process

- **Status**: Implemented
- **Category**: Process
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-17
- **Extended by**: [RFD 100][tickets]

## Summary

This document establishes the Request for Discussion (RFD) process for the JP
project.
RFDs are short design documents that describe a significant change before
implementation begins: a new feature, an architectural shift, a process change.
The goal is to think clearly, communicate intent, and invite feedback early.

## Motivation

JP already has a `docs/architecture` directory with documents that describe
system designs in detail.
Several of these (query stream pipeline, structured output, stateful tool
inquiries, wasm tools) are effectively design proposals: they describe what we
intend to build, why, and how.
But they lack a formal lifecycle.
There is no way to distinguish a proposal under discussion from an accepted
design, or to track when a document was superseded.

We want a lightweight process that:

1. Gives design documents a clear lifecycle (draft → discussion → accepted →
   implemented; or abandoned / superseded).
2. Lowers the barrier to proposing ideas, so rough thoughts are welcome.
3. Creates a searchable record of decisions and their rationale.
4. Works naturally with our existing Git + pull request workflow.

We do not want a process that adds bureaucracy, requires approvals from
committees, or discourages people from writing things down.

## Principles

The RFD process is guided by a few core beliefs, drawn from the IETF's original
RFC spirit and refined for a small, fast-moving open-source project.

### Timely over polished

A rough document written now is more valuable than a perfect document written
never.
RFDs are encouraged to be concise and direct.
An RFD can be a single page.
Grammar and formatting matter less than clarity of thought.

> "Notes are encouraged to be timely rather than polished.
> Philosophical positions without examples or other specifics, specific
> suggestions or implementation techniques without introductory or background
> explication, and explicit questions without any attempted answers are all
> acceptable.
> The minimum length for a note is one sentence."
>
> [RFC 3], Steve Crocker, 1969

### Opinionated with options

An RFD should propose a specific solution, not present an open-ended menu of
choices.
The author's job is to navigate the problem space, evaluate alternatives, and
land on a recommendation.
Readers should understand *what* is proposed, *why* it was chosen, and *what
else* was considered.

Ambiguity creates unproductive discussion.
If you're unsure about the solution, that's fine.
State what you know, what you don't, and what you recommend given current
information.
Spike with code if you need to build confidence.

### Small scope

Keep RFDs focused.
One document, one topic.
If a change has multiple independent parts, write multiple RFDs.
A focused document is easier to review, easier to discuss, and leads to faster
consensus.

Use the "Non-Goals" or "Future Work" sections to acknowledge related concerns
you're deliberately deferring.
This signals awareness without bloating the current proposal.

### Permanent record

RFDs are never deleted.
If an idea is abandoned, the document is marked as such with a brief
explanation.
If a design is superseded, the old document links to the new one.
This preserves the reasoning behind past decisions and helps future contributors
understand why things are the way they are.

## When to Write an RFD

The test is one question:

> Does this change create a contract that is expensive to reverse?

A contract is anything others depend on once it ships: an on-disk format, a CLI
surface, a config key, an event shape, a cross-crate boundary, a protocol.
Expensive to reverse means changing it later costs a migration, a deprecation,
or a broken user script.

If the answer is yes, write an RFD.
If the answer is no, open a ticket.

Wanting structured feedback is not a reason to write an RFD.
That is a reason to have a conversation.
A conversation costs an hour.
An RFD costs a review cycle, a permanent number, and a document the project
maintains forever.

Write an RFD when the change includes one of these:

- a new public interface, protocol, persisted format, or config contract
- a change to data ownership or a crate boundary
- a process or policy decision that is hard to reverse

Keep it as a ticket when:

- the implementation follows an established pattern
- the decision is local and reversible
- it is a bug, a small feature, or a contained refactor
- the design questions can be settled during code review

Do NOT write an RFD for:

- Bug fixes
- Performance improvements with no architectural change
- Code reorganization that doesn't change behavior
- Documentation updates

When you are unsure, open the ticket.
A ticket that turns out to need an RFD is cheap to promote, and the ticket's
problem statement carries straight into it.
An RFD that turns out to have been a ticket has already cost the review cycle.

**Only a human escalates a ticket to an RFD.** An assistant can recommend it and
name the criterion it thinks applies.
But reaching for an RFD is not how an assistant resolves its own uncertainty.
The answer to "I am not sure how big this is" is a ticket.

## RFD Lifecycle

An RFD moves through the following states:

```
Draft → Discussion → Accepted  → Implemented ┐
                   ↘ Abandoned ↘ Superseded ◄┘
```

Most RFDs follow the happy path: Draft → Discussion → Accepted → Implemented.
The remaining states handle the less common cases:

- **Abandoned**: The idea was rejected or withdrawn during discussion.
- **Superseded**: An accepted or implemented design was later replaced by a new
  RFD.

### Draft

The author is actively writing the document.
It may be incomplete, have open questions, or change shape entirely.
Drafts live on a branch and are not yet ready for formal review, but early
feedback from collaborators is encouraged.

Drafts do not have a permanent number.
They live in `docs/rfd/drafts/` and are named `DNN-slug.md`, where `D` stands
for "Draft" and `NN` is a two-digit draft slot, `D01` to `D99`.
This prevents speculative cross-draft dependencies and avoids number gaps from
abandoned drafts.
The permanent number is assigned when the RFD advances to Discussion.

Drafts are not published to the documentation site.
The `drafts/` directory is excluded from the build, so drafts exist in the
repository for committing, reviewing, and iterating without the pressure of a
public page.
Abandoned drafts cost nothing beyond the disk space they occupy.
Published RFDs must not link to drafts; any such link fails the docs build.

Drafts cannot be superseded.
A draft replaced before it advances to Discussion is **deleted**, not superseded.
Supersedes only applies once an RFD reaches Accepted, because before that there
is no design to preserve.
A draft the author has dropped can be deleted, or abandoned via `just
rfd-abandon` when the rejected idea is worth recording.
The choice is the author's.

### Discussion

The RFD is complete enough to review.
When the author runs `just rfd-promote D01` (using the draft ID), the tooling:

1. Assigns the next available sequential permanent number.
2. Moves the file from `docs/rfd/drafts/D01-slug.md` to `docs/rfd/042-slug.md`
   (for example).
3. Updates the heading in the file.

A pull request is opened to merge the document into `main`.
Discussion happens on the pull request.
The author incorporates feedback and iterates on the document.

There is no fixed timeline for discussion.
For most RFDs, a few days should suffice.
If no one has reviewed your RFD after 48 hours, ask someone directly.
If discussion stalls, a synchronous conversation (call, chat) can help break the
deadlock.

### Accepted

Discussion has converged and the pull request is merged.
The RFD represents the agreed-upon direction.
Implementation can begin.

An accepted RFD is not immutable, and how freely it changes depends on its
category.

A **Design** RFD describes one change to the product.
Keep it in sync with the code through minor edits.
When the feature is redesigned or dropped, write a new RFD that supersedes it.
The original recorded a decision that no longer holds, and that record is worth
keeping.

A **Process** or **Guide** RFD describes how the project works, and is expected
to change as the project does.
This document is about the RFD process and always will be; when the process
changes, this document changes with it, with no supersede chain.
When one part of the process grows enough to deserve its own document, write a
new RFD and record the relationship with `just rfd-extend`.

Neither rule is absolute.
A process or guide that has become obsolete rather than merely outdated can be
abandoned, or superseded, when that is the clearer record.

### Implemented

The feature or change described in the RFD has been fully implemented.
This is a bookkeeping state.
It signals that the document describes the current system, not just a plan.

### Superseded

The design in this RFD has been replaced by a newer RFD.
The original document remains in the repository as a historical record.
Its metadata is updated with a **Superseded by** link pointing to the
replacement, and the new RFD carries a **Supersedes** link pointing back.

Superseded is distinct from Abandoned: a superseded RFD was accepted and may
have been partially or fully implemented, but a later design replaced it.
An abandoned RFD was never accepted or implemented.

An RFD can be superseded from either the Accepted or Implemented state.
Drafts cannot be superseded.
A draft replaced before promotion is deleted; see [Draft](#draft) above.

### Abandoned

The idea was considered and deliberately set aside.
The document remains in the repository with a brief note explaining why.
Common reasons: the problem was solved differently, priorities changed, or the
approach turned out to be infeasible.

An abandoned RFD opens with a standard notice block:

```markdown
> [!IMPORTANT]
> This RFD is **Abandoned**.
>
> {one-line reason}
>
> {what was carried forward and where it lives now, if anything.}
>
> The original text below is preserved for historical context.
```

If the RFD was split into other RFDs, name the active successors.
If portions were superseded by a later RFD, name it.
If nothing was carried forward, omit the second paragraph.

## Document Format

### Filename

Drafts live under `docs/rfd/drafts/` and use a `DNN` prefix (`D` for Draft, `NN`
a two-digit slot):

```
docs/rfd/drafts/D01-short-title.md
```

When promoted to Discussion, the tooling assigns a permanent number and moves
the file up to `docs/rfd/`:

```
docs/rfd/042-short-title.md
```

- `DNN` is used for drafts; a zero-padded sequential number (001, 002, ...) is
  assigned at Discussion.
- `short-title` is a lowercase, hyphen-separated slug.
  Keep it short but descriptive.
- Numbers are never reused.
  If an RFD is abandoned, its number is retired.

### Templates

Every RFD has a **Category** that describes its purpose.
Each category has a corresponding template:

| Category     | Template      | Use when                            |
| ------------ | ------------- | ----------------------------------- |
| **Design**   | [`000-design-template.md`] | Proposing a feature or              |
|              |               | architectural change that needs a   |
|              |               | design.                             |
| **Decision** | [`000-decision-template.md`] | Recording a decision: a technology  |
|              |               | choice, a convention, a policy.     |
| **Guide**    | [`000-guide-template.md`] | How-tos, reference material, and    |
|              |               | contributor-facing documentation.   |
| **Process**  | [`000-process-template.md`] | How the project operates:           |
|              |               | workflows, policies, values.        |

All categories share the same numbering scheme, directory, lifecycle, and review
process.
The difference is purpose: a Design has a full design section and implementation
plan; a Decision has concise context and consequences; a Guide or Process
document is free-form.

**The templates are starting points, not constraints.** Structure the document
however it reads best.
The only requirement is the metadata header (Status, Category, Authors, Date) so
the tooling and lifecycle work.
Delete the template sections that don't apply, add sections that do, or write
something entirely free-form.

To create a new draft:

```sh
just rfd-draft design "My Feature Title"        # design proposal
just rfd-draft decision "Use TOML for Config"   # decision record
just rfd-draft guide "Attachment Handler Guide" # how-to / reference
just rfd-draft process "Release Process"        # workflow / policy
```

This copies the appropriate template to
`docs/rfd/drafts/D01-my-feature-title.md` (or the next available draft slot),
fills in the title, author, date, and category, and sets the status to
**Draft**.
The draft prefix is replaced with a permanent number and the file moves up to
`docs/rfd/` when the RFD is promoted to Discussion.

### Document sections

Not all sections are required for every RFD.
Omit the ones that genuinely don't apply, but think twice before skipping one.
Every section can be brief.
A one-sentence Alternatives section is better than no Alternatives section.

### Metadata header

All categories use the same metadata:

```markdown
- **Status**: Draft | Discussion | Accepted | Implemented | Superseded | Abandoned
- **Category**: Design | Decision | Guide | Process
- **Authors**: Name <email> (or GitHub handle)
- **Date**: YYYY-MM-DD
- **Extends**: RFD NNN (if this RFD builds on another)
- **Extended by**: RFD NNN (if another RFD builds on this one)
- **Requires**: RFD NNN (if this RFD depends on another)
- **Required by**: RFD NNN (if another RFD depends on this one)
- **Supersedes**: RFD NNN (if applicable)
- **Superseded by**: RFD NNN (if applicable)
- **Over budget**: reason (if the RFD exceeds its prose budget)
```

Implementation progress is tracked in [tickets], not in the metadata header.
When `just rfd-promote` advances an RFD to Accepted, it offers to turn each
phase of the Implementation Plan into a ticket carrying `Implements: NNN`.
The phases are read out of the document by `jp`, so the tickets match the plan
rather than a fixed template.
Whoever accepts the prompt reviews them before they land.

Accepting an RFD records an agreed direction, not a commitment to start
building, which is why the tickets are offered rather than created.
Decline the prompt and file them later with `just ticket-create` when the work
starts.

A handful of older RFDs still carry a `Tracking Issue` field pointing at a
GitHub issue.
The field is retired: nothing writes it, and the lifecycle recipes only read it
to remind you to close the issue when such an RFD is superseded or abandoned.

The `Extends` and `Extended by` fields capture design-lineage relationships:
"this RFD builds on that one's design."
They are maintained by `just rfd-extend`.
Unlike `Supersedes`, an extending RFD builds on its predecessor, and the
original remains valid and in effect.

The `Requires` and `Required by` fields capture hard dependencies: "this RFD
cannot be Accepted (or Implemented) until that one is."
They are maintained by `just rfd-require`.

`Requires` exists only to gate promotion on an unbuilt dependency.
Once a target reaches `Implemented` the dependency is satisfied for good, so a
`Requires` entry is never recorded against one.
When an RFD itself reaches `Implemented`, `just rfd-promote` strips its
`Requires` entries and the matching `Required by` back-links on the targets.
By that point the gate guarantees every target is `Implemented` or `Superseded`.
The docs build rejects any published RFD that lists a `Requires` on an
`Implemented` target.
Use `Extends` instead when the relationship is design lineage worth recording
past implementation.

**Both relationships participate in the same gate.** An extension is a kind of
dependency (`Extends ⊆ Requires`): if A extends B, then A also depends on B.
So `rfd-promote` and the docs build enforce both fields uniformly:

- Promoting an RFD from **Discussion to Accepted** requires every entry in
  `Requires` *or* `Extends` to be at status `Accepted`, `Implemented`, or
  `Superseded`.
- Promoting from **Accepted to Implemented** requires every entry to be at
  status `Implemented` or `Superseded`.

This lets RFDs be designed in parallel (multiple RFDs in `Accepted` at once)
while preventing claims of design completion that rest on an unbuilt foundation.
Cycles in the union graph are refused at write time and by the docs build.

**Don't list the same target in both `Extends` and `Requires`.** Extension
implies the dependency, so listing the same target twice is redundant.
Pick `Extends` when the relationship is design lineage; pick `Requires` when it
is only execution prerequisite.
The recipes refuse to write a duplicate, and the docs build refuses to publish
one.

Drafts may participate in the dependency graph, but back-links from non-draft
RFDs are suppressed: a published RFD never lists a draft under `Required by` or
`Extended by`.
When a draft is promoted, missing back-links on its dependency targets are
filled in automatically by `rfd-promote`.

### Writing Style

- **Use present tense.** "This RFD describes..." not "This RFD was created to
  describe..."
- **Be direct.** State what you propose and why, without hedging.
- **Use concrete examples.** A code snippet or data flow diagram is worth a
  paragraph of abstract description.
- **Define terms.** If you introduce a concept, define it where it first
  appears.
- **Keep sentences short.** A long sentence is usually two sentences and a comma
  splice.
- **One word, one meaning, across the document.** If the code calls it a `Turn`,
  it is a Turn every time.
  Rotating through synonyms to avoid repetition costs the reader.
- **Prefer deletion.** Clarify existing prose before adding a caveat.
  An RFD does not need to record every reachable edge case or implementation
  choice.
- **Keep it short.** If a document has grown past five or six pages, it is
  usually two RFDs.
- **Reference-style links only.** `[RFD 001]` in the body, the target defined at
  the bottom.

## Process

### Creating an RFD

1. Create a branch for your work.
2. Run `just rfd-draft CATEGORY Your Title` to generate the file from the
   appropriate template.
   Category is one of: `design`, `decision`, `guide`, `process`.
   The file lands under `docs/rfd/drafts/` with a `DNN` prefix.
3. Write your proposal.
4. Push your branch and iterate until you are ready for feedback.

An assistant can also author and refine an RFD for you, under a bounded pipeline
with its own budgets, checks, and review protocol.
[RFD 003] covers what you remain responsible for, and [RFD 099] covers how the
pipeline runs.
You own the content of an RFD whatever tools you used to write it.

### Opening for Discussion

1. Run `just rfd-promote D01` to advance the status to **Discussion**.
   This assigns a permanent number and renames the file.
2. Open a pull request to merge your branch into `main`.
3. Tag reviewers who have context on the problem area.
4. Engage with feedback.
   Update the document as the discussion evolves.

### Accepting an RFD

1. When discussion converges, run `just rfd-promote NNN` to advance the status
   to **Accepted**.
   The promotion is gated on the RFD's `Requires` *and* `Extends` fields: every
   entry in either field must be at status `Accepted`, `Implemented`, or
   `Superseded`.
2. Accept the prompt to file a ticket per implementation phase, or decline it
   and file them when the work starts.
3. Merge the pull request.

### After Acceptance

- **Minor updates**: Edit the document directly on `main` via a standard pull
  request.
  No new RFD number needed.
- **Significant changes**: Write a new RFD that supersedes the original.
- **Implementation complete**: Run `just rfd-promote NNN` to advance the status
  to **Implemented**.
- **Design extended**: When a new RFD builds on this one, run `just rfd-extend
  NNN MMM` to record the relationship in both documents.
- **Design depended on**: When a new RFD requires this one as a hard
  prerequisite, run `just rfd-require NNN MMM` (NNN requires MMM) to record the
  dependency.
  The relationship gates promotion of the dependent RFD.
- **Design superseded**: Write a new RFD, then run `just rfd-supersede NNN MMM`
  to mark the old RFD as superseded and cross-link both documents.
- **Idea abandoned**: Run `just rfd-abandon NNN "reason"` to mark the RFD as
  abandoned with an explanation.

### Tooling

All RFD commands are in the `rfd` group.
Run `just --list --group rfd` to see them.

| Command                         | Description                              |
| ------------------------------- | ---------------------------------------- |
| `just rfd-draft CATEGORY TITLE` | Create a blank draft under `drafts/`.    |
| `just rfd-promote NNN [REASON]` | Advance status. Draft → Discussion       |
|                                 | assigns a number; Discussion → Accepted  |
|                                 | offers phase tickets. REASON records an  |
|                                 | over-budget exemption.                   |
| `just rfd-extend NNN MMM`       | Record that RFD MMM extends RFD NNN,     |
|                                 | updating both. Accepts draft IDs (DNN)   |
|                                 | on either side.                          |
| `just rfd-require NNN MMM`      | Record that RFD NNN requires RFD MMM,    |
|                                 | updating both. Cycles are refused.       |
|                                 | Accepts draft IDs (DNN) on either side.  |
| `just rfd-supersede NNN MMM`    | Mark RFD NNN as superseded by RFD MMM,   |
|                                 | updating both.                           |
| `just rfd-abandon NNN REASON`   | Mark RFD NNN as abandoned with the given |
|                                 | reason.                                  |
| `just rfd-renumber NNN [MMM]`   | Renumber an RFD (draft or published) to  |
|                                 | `MMM`, or the next available id, and     |
|                                 | rewrite all cross-references.            |
| `just rfd-grep TERM`            | Search across all RFD documents using    |
|                                 | `rg`.                                    |
| `just rfd-list [CATEGORY]`      | List all RFDs (including DNN-prefixed    |
|                                 | drafts), optionally filtered by          |
|                                 | category.                                |

This table covers the lifecycle.
The assistant-authoring pipeline adds more commands to the same group, and
[RFD 099] documents them.

## Relationship to Architecture Documents

The existing `docs/architecture/` directory contains detailed technical
descriptions of implemented systems.
These serve a different purpose than RFDs:

|               | RFDs (`docs/rfd/`)                  | Architecture Docs (`docs/architecture/`) |
| ------------- | ----------------------------------- | ---------------------------------------- |
| **Purpose**   | Propose a change                    | Describe the current system              |
| **Lifecycle** | Draft → Accepted → Implemented      | Living documents, updated as the system  |
|               |                                     | evolves                                  |
| **Audience**  | Contributors deciding what to build | Contributors understanding what exists   |
| **Scope**     | A specific change or feature        | A subsystem or cross-cutting concern     |

The typical flow: an RFD proposes a design, gets accepted, and is implemented.
Once implemented, the relevant architecture documents are updated to reflect the
new state of the system.
The RFD remains as a historical record of the decision.

Over time, some existing architecture documents may be retroactively referenced
by RFDs, or new architecture documents may be created as companions to accepted
RFDs.
The two directories complement each other.

## FAQ

### What if I'm not sure about the solution?

Write what you know.
State the options you see and which one you lean toward.
Use the "Risks and Open Questions" section to flag uncertainty.
A draft with acknowledged unknowns is more useful than no document at all.

If you need to experiment first, do that.
Write the RFD after you've spiked and have a clearer picture.

### How detailed should the design section be?

Detailed enough that a reviewer can evaluate the approach without reading the
implementation code.
Not so detailed that it becomes the implementation spec.
RFDs describe the "what" and "why" at an architectural level; the code is the
"how" at an implementation level.

For JP specifically, the existing architecture documents provide a good
reference for the level of detail expected: design goals as tables, data flow
descriptions, component responsibilities, migration paths.

### Can I update an accepted RFD?

Yes, and how freely depends on its category.
See [Accepted](#accepted).

### What about the existing architecture documents?

They stay where they are.
The architecture directory describes the system as it is.
The RFD directory captures proposals and decisions.
Both are valuable.
See [Relationship to Architecture
Documents](#relationship-to-architecture-documents).

### Do I need approval to merge an RFD?

Follow the project's normal pull request process.
An RFD should be reviewed by at least one other contributor with relevant
context before merging.
The goal is consensus, not a formal sign-off process.

### What if my document doesn't fit either template?

Use whatever structure makes sense.
The templates are suggestions to help you get started, not a format you must
follow.
Policy documents, values statements, and process guidelines have their own
natural shape.
The only hard requirement is the metadata header (Status, Authors, Date) at the
top of the file, so the lifecycle tooling works.
See [RFD 002] for an example of a free-form RFD.

[RFC 3]: https://datatracker.ietf.org/doc/html/rfc3
[RFD 002]: 002-using-llms.md
[RFD 003]: 003-jp-assisted-rfds.md
[RFD 099]: 099-rfd-authoring-and-review-pipeline.md
[`000-decision-template.md`]: 000-decision-template.md
[`000-design-template.md`]: 000-design-template.md
[`000-guide-template.md`]: 000-guide-template.md
[`000-process-template.md`]: 000-process-template.md
[tickets]: 100-in-repo-ticket-tracking.md
