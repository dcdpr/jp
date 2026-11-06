# RFD 001: The JP RFD Process

- **Status**: Implemented
- **Category**: Process
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-17
- **Extended by**: [RFD 100][tickets], [RFD 116]
- **Summary**: Establishes a lightweight Request for Discussion process for
  proposing and tracking design decisions with clear lifecycle states.

## Summary

This document establishes the Request for Discussion (RFD) process for the JP
project.
RFDs are short design documents that describe a significant change, such as a
new feature, an architectural shift, or a process change, before implementation
begins.
The goal is to think clearly, communicate intent, and invite feedback early.

## Motivation

JP's `docs/architecture` directory contains detailed system design documents.
Several of these (query stream pipeline, structured output, stateful tool
inquiries, wasm tools) are effectively design proposals: they describe what we
intend to build, why, and how.
However, they lack a formal lifecycle: there is no way to distinguish a proposal
under discussion from an accepted design, or to track when a document has been
superseded.

We want a lightweight process that:

1. Gives design documents a clear lifecycle (draft → discussion → accepted →
   implemented, or abandoned / superseded).
2. Lowers the barrier to proposing ideas, welcoming rough thoughts.
3. Creates a searchable record of decisions and their rationale.
4. Fits our existing Git and pull request workflow.

We do not want a process that adds bureaucracy, requires committee approvals, or
discourages people from writing things down.

## Principles

The RFD process is guided by a few core beliefs, drawn from the IETF's original
RFC spirit and refined for a small, fast-moving open-source project.

### Timely over polished

A rough document written now is more valuable than a perfect document written
never.
Keep RFDs concise and direct; a single page is enough.
Clarity of thought matters more than grammar and formatting.

> "Notes are encouraged to be timely rather than polished.
> Philosophical positions without examples or other specifics, specific
> suggestions or implementation techniques without introductory or background
> explication, and explicit questions without any attempted answers are all
> acceptable.
> The minimum length for a note is one sentence."
>
> -- [RFC 3], Steve Crocker, 1969

### Opinionated with options

Propose a specific solution rather than an open-ended menu of choices.
The author's job is to navigate the problem space, evaluate alternatives, and
land on a recommendation.
Readers should understand *what* is proposed, *why* it was chosen, and *what
else* was considered.

Ambiguity creates unproductive discussion.
When uncertain about the solution, state what you know, what you don't, and what
you recommend given current information.
Spike with code if you need to build confidence.

### Small scope

Keep each RFD focused on a single topic.
If a change has multiple independent parts, split it across multiple RFDs.
Focused documents are easier to review and discuss, and reach consensus faster.

Use the "Non-Goals" or "Future Work" sections to note deferred concerns without
bloating the proposal.

### Permanent record

Published RFDs are never deleted.
Abandoned documents are marked with a brief explanation, and superseded designs
link to their replacements.
This preserves the reasoning behind past decisions for future contributors.

## When to Write an RFD

Write an RFD when:

- Adding a new feature that affects the architecture or public interface
- Making a significant change to the data model or event system
- Introducing a new dependency, protocol, or integration pattern
- Changing the build, release, or contribution process
- Removing a feature or deprecating an interface
- Proposing a large refactoring effort
- Seeking structured feedback on a change before investing in code

Do not write an RFD for:

- Bug fixes
- Performance improvements with no architectural change
- Code reorganization that doesn't change behavior
- Small feature additions that fit within established patterns
- Documentation updates

When in doubt, start writing.
If it turns out to be unnecessary, you'll know quickly.
The cost of an unnecessary RFD is low; the cost of a misaligned implementation
is high.

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

A draft is a work in progress on a branch.
It may be incomplete, have open questions, or change significantly before formal
review, though early feedback from collaborators is welcome.

Drafts live in `docs/rfd/drafts/DNN-slug.md`, where `D` stands for "Draft" and
`NN` is a two-digit slot from `01` to `99`.
Deferring permanent numbering until the RFD advances to Discussion prevents
speculative cross-draft dependencies and avoids number gaps from abandoned
drafts.

The `drafts/` directory is excluded from the documentation site build, so
authors can commit and iterate in the repository without publishing a public
page.
Published RFDs must not link to drafts; any such link fails the docs build.

Drafts cannot be superseded, because `Supersedes` only applies once an RFD
reaches Accepted and has an agreed design to preserve.
A draft replaced before advancing to Discussion is **deleted**.
An unpursued draft can either be deleted or, if the rationale for rejecting the
idea is worth keeping, marked as abandoned with `jp rfd abandon`.

### Discussion

The RFD is complete enough to review.
When the author runs `just rfd-promote DNN` (using the draft ID), the tooling:

1. Assigns the next available sequential permanent number.
2. Moves the file from `docs/rfd/drafts/DNN-slug.md` to `docs/rfd/042-slug.md`
   (for example).
3. Updates the heading in the file.

The author then opens a pull request to merge the document into `main`, where
reviewers discuss the proposal and the author iterates on feedback.

Discussion has no fixed timeline, though a few days suffice for most RFDs.
If no one reviews the RFD within 48 hours, ask a reviewer directly; if
discussion stalls, use a synchronous call or chat to resolve the deadlock.

### Accepted

Once discussion converges and the pull request merges, the RFD represents the
agreed direction and implementation can begin.

An accepted RFD is not immutable.
If implementation reveals minor issues, edit the document in place; for
significant design changes, write a new RFD that supersedes the original.

### Implemented

The feature or change described in the RFD is complete.
This bookkeeping state signals that the document describes the current system
rather than a plan.

### Superseded

A newer RFD has replaced this design.
The original document remains in the repository as a historical record, with a
**Superseded by** link in its metadata and a matching **Supersedes** link in the
replacement RFD.

Only Accepted or Implemented RFDs can be superseded.
Unlike an Abandoned RFD, which was never accepted, a Superseded RFD was accepted
and possibly implemented before a later design replaced it.
A draft replaced before promotion is deleted instead (see [Draft](#draft)).

### Abandoned

The idea was considered and deliberately set aside, because the problem was
solved differently, priorities changed, or the approach proved infeasible.
The document remains in the repository with a brief note explaining why.

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

If the RFD was split into other RFDs or partially carried into a later RFD, name
the active successors in the second paragraph; if nothing was carried forward,
omit that paragraph.

## Document Format

### Filename

Drafts live under `docs/rfd/drafts/` with a two-digit `DNN` draft prefix:

```
docs/rfd/drafts/DNN-short-title.md
```

Promoting a draft to Discussion assigns a zero-padded sequential number (`001`,
`002`, ...) and moves the file to `docs/rfd/`:

```
docs/rfd/042-short-title.md
```

`short-title` is a short, descriptive, lowercase, hyphen-separated slug.
Permanent numbers are never reused; if an RFD is abandoned, its number is
retired.

### Templates

Every RFD has a **Category** that describes its purpose and selects its
template:

| Category     | Template      | Use when                           |
| ------------ | ------------- | ---------------------------------- |
| **Design**   | [`000-design-template.md`] | Proposing a feature or             |
|              |               | architectural change that needs a  |
|              |               | design.                            |
| **Decision** | [`000-decision-template.md`] | Recording a decision: a technology |
|              |               | choice, convention, or policy.     |
| **Guide**    | [`000-guide-template.md`] | Writing how-tos, reference         |
|              |               | material, or contributor-facing    |
|              |               | documentation.                     |
| **Process**  | [`000-process-template.md`] | Defining project workflows,        |
|              |               | policies, or values.               |

All categories share the same numbering scheme, directory, lifecycle, and review
process.
The difference is structure: a Design includes a full design section and
implementation plan, a Decision records concise context and consequences, and a
Guide or Process document is free-form.

**The templates are starting points, not constraints.** Structure the document
however it reads best.
Only the metadata header (Status, Category, Authors, Date) is required by the
lifecycle tooling.
Delete template sections that do not apply, add sections that do, or write a
free-form document.

To create a new draft:

```sh
jp rfd draft design "My Feature Title"        # design proposal
jp rfd draft decision "Use TOML for Config"   # decision record
jp rfd draft guide "Attachment Handler Guide" # how-to / reference
jp rfd draft process "Release Process"        # workflow / policy
```

`jp rfd draft` copies the category template to
`docs/rfd/drafts/DNN-my-feature-title.md` using the next available draft slot,
populates the title, author, date, and category, and sets the status to
**Draft**.

#### Document sections

Omit template sections that genuinely do not apply, but think twice before
skipping one.
Sections can be brief: a one-sentence Alternatives section is better than none.

#### Metadata header

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
- **Summary**: One sentence, written once the RFD has a permanent number
```

`Summary` is always the last field in the header.

Implementation progress is tracked in [tickets], not in the metadata header.
When `just rfd-promote` advances an RFD from Discussion to Accepted, it offers
to create a tracking ticket labelled `tracking-rfd=NNN`, using the RFD's
Implementation Plan section as the ticket description so you can review it
before creation.

Because accepting an RFD records an agreed direction rather than a commitment to
start building immediately, creating the tracking ticket is optional at
promotion time.
You can decline the prompt and create the ticket later with `jp rfd track NNN`.
An RFD is considered in development while its tracking ticket has status In
Progress; move the ticket on the [ticket board] when work starts, or run `jp rfd
start NNN` to create the ticket (if needed) and mark it In Progress.
Additional tickets for the RFD use the `relevant-rfd=NNN` label, and the
[priority board] links each RFD to all of its tickets.

Some older RFDs carry a retired `Tracking Issue` field pointing to a GitHub
issue.
New RFDs do not use this field; `jp rfd` only reads it to remind you to close
the GitHub issue when the RFD is superseded or abandoned.

The `Extends` and `Extended by` fields record design lineage and are maintained
by `jp rfd extend`.
Unlike `Supersedes`, an extending RFD builds on its predecessor while leaving
the original design in effect.

The `Requires` and `Required by` fields record hard prerequisites and are
maintained by `jp rfd require`: the dependent RFD cannot be Accepted or
Implemented until its prerequisite reaches that state.

`Requires` only gates promotion on unbuilt dependencies.
Once a target RFD reaches `Implemented`, the dependency is permanently
satisfied, so `jp rfd require` and the docs build reject `Requires` links to
`Implemented` targets.
When an RFD reaches `Implemented`, `just rfd-promote` strips its `Requires`
entries and the corresponding `Required by` back-links from its targets.
Use `Extends` instead when design lineage should remain recorded after
implementation.

**Both relationships share the same promotion gate.** Because extending a design
also depends on it (`Extends ⊆ Requires`), `rfd-promote` and the docs build
enforce both fields identically:

- Promoting an RFD from **Discussion to Accepted** requires every entry in
  `Requires` *or* `Extends` to be at status `Accepted`, `Implemented`, or
  `Superseded`.
- Promoting from **Accepted to Implemented** requires every entry to be at
  status `Implemented` or `Superseded`.

This lets contributors design RFDs in parallel (with multiple RFDs in `Accepted`
at once) while preventing an RFD from being marked complete before its
dependencies are built.
`jp rfd` and the docs build reject cycles in the combined dependency graph.

**Do not list the same target in both `Extends` and `Requires`.** Because
`Extends` implies `Requires`, listing a target in both fields is redundant; `jp
rfd` and the docs build reject duplicates.
Use `Extends` for design lineage and `Requires` for a pure execution
prerequisite.

Drafts can declare `Requires` and `Extends` links, but published RFDs never list
drafts under `Required by` or `Extended by`.
Promoting a draft to Discussion with `rfd-promote` adds the missing back-links
to its targets.

#### Summary

Every published RFD requires a one-sentence `Summary` field as the last entry in
its metadata header.
The index pages and the [priority board] display this line, and the docs build
rejects any published RFD without it.
Drafts do not need a `Summary` field until they are promoted to Discussion.

```markdown
- **Summary**: Establishes a lightweight process for proposing and tracking
  design decisions.
```

Unlike the body's `## Summary` section, which introduces the proposal in two or
three sentences, the metadata `Summary` is a single line for index listings.
The docs build only verifies that the field is present, so reviewers should
check that edits to an RFD keep its `Summary` field accurate.

### Writing Style

- **Use present tense.** "This RFD describes..." not "This RFD was created to
  describe..."
- **Be direct.** Avoid hedging language such as "it seems," "probably," or "it
  might be worth considering."
  State what you propose and why.
- **Use concrete examples.** A code snippet or data flow diagram is worth a
  paragraph of abstract description.
- **Define terms.** If you introduce a concept, define it where it first
  appears.
- **Keep it short.** If an RFD exceeds 5-6 pages (roughly 2000 words), consider
  splitting it into smaller proposals.

## Process

### Creating an RFD

1. Create a branch for your work.
2. Run `jp rfd draft CATEGORY Your Title` (where `CATEGORY` is `design`,
   `decision`, `guide`, or `process`) to generate
   `docs/rfd/drafts/DNN-your-title.md` from the category template.
3. Write the proposal.
4. Push your branch and iterate until the draft is ready for feedback.

### Opening for Discussion

1. Run `just rfd-promote DNN` to advance the status to **Discussion**, which
   assigns a permanent number and renames the file.
2. Add the `Summary` field to the metadata header.
   The docs build rejects a published RFD without one.
3. Open a pull request to merge your branch into `main`.
4. Tag reviewers with context on the problem area.
5. Incorporate feedback and update the document as the discussion evolves.

### Accepting an RFD

1. When discussion converges, run `just rfd-promote NNN` to advance the status
   to **Accepted**.
   Every entry in the RFD's `Requires` and `Extends` fields must already be at
   status `Accepted`, `Implemented`, or `Superseded`.
2. Accept the prompt to file the RFD's tracking ticket, or decline it and file
   the ticket when work starts.
3. Merge the pull request.

### After Acceptance

- **Minor updates**: Edit the document directly on `main` via a standard pull
  request; no new RFD number is needed.
- **Significant changes**: Write a new RFD that supersedes the original.
- **Implementation complete**: Run `just rfd-promote NNN` to advance the status
  to **Implemented**.
- **Design extended**: When a new RFD builds on this one, run `jp rfd extend MMM
  NNN` (MMM extends NNN) to record the relationship in both documents.
- **Design depended on**: When a new RFD requires this one as a hard
  prerequisite, run `jp rfd require NNN MMM` (NNN requires MMM) to record the
  dependency in both documents.
- **Design superseded**: Write a new RFD, then run `jp rfd supersede NNN MMM` to
  mark the old RFD as superseded and cross-link both documents.
- **Idea abandoned**: Run `jp rfd abandon NNN "reason"` to mark the RFD as
  abandoned with an explanation.

### Tooling

The `jp-rfd` command plugin ([RFD 116]) provides the `jp rfd` subcommands; run
`jp rfd --help` to list them.
Use `just rfd-promote` to promote RFDs: it wraps `jp rfd promote` and, when
promoting a draft to Discussion, archives any review and triage conversations
named after the draft ID.

| Command                       | Description                              |
| ----------------------------- | ---------------------------------------- |
| `jp rfd draft CATEGORY TITLE` | Create a new draft under `drafts/`.      |
| `just rfd-promote NNN`        | Advance status. Draft → Discussion       |
|                               | assigns a number; Discussion → Accepted  |
|                               | offers the tracking ticket.              |
| `jp rfd track NNN`            | File or refresh the RFD's tracking       |
|                               | ticket, printing its ID.                 |
| `jp rfd start NNN`            | Mark the RFD in development by moving    |
|                               | its tracking ticket to In Progress.      |
| `jp rfd stop NNN`             | Move the tracking ticket back to Todo.   |
| `jp rfd extend NNN MMM`       | Record that RFD NNN extends RFD MMM,     |
|                               | updating both. Accepts draft IDs (DNN)   |
|                               | on either side.                          |
| `jp rfd require NNN MMM`      | Record that RFD NNN requires RFD MMM,    |
|                               | updating both. Rejects cycles. Accepts   |
|                               | draft IDs (DNN) on either side.          |
| `jp rfd supersede NNN MMM`    | Mark RFD NNN as superseded by RFD MMM,   |
|                               | updating both.                           |
| `jp rfd abandon NNN REASON`   | Mark RFD NNN as abandoned with the given |
|                               | reason.                                  |
| `jp rfd renumber NNN [MMM]`   | Renumber an RFD (draft or published) to  |
|                               | `MMM`, or the next available ID, and     |
|                               | rewrite all cross-references.            |
| `rg TERM docs/rfd/`           | Search across all RFD documents.         |
| `jp rfd list [CATEGORY]`      | List RFDs in priority order, including   |
|                               | drafts, optionally filtered by category. |

## Relationship to Architecture Documents

The `docs/architecture/` directory contains detailed technical descriptions of
implemented systems, which serve a different purpose than RFDs:

|               | RFDs (`docs/rfd/`)                  | Architecture Docs (`docs/architecture/`) |
| ------------- | ----------------------------------- | ---------------------------------------- |
| **Purpose**   | Propose a change                    | Describe the current system              |
| **Lifecycle** | Draft → Accepted → Implemented      | Living documents, updated as the system  |
|               |                                     | evolves                                  |
| **Audience**  | Contributors deciding what to build | Contributors understanding what exists   |
| **Scope**     | A specific change or feature        | A subsystem or cross-cutting concern     |

Once an accepted RFD is implemented, contributors update the relevant
architecture documents to reflect the new system state, while the RFD remains as
the historical record of the decision.
The two directories complement each other: RFDs may reference existing
architecture documents, and accepted RFDs may introduce new ones.

## FAQ

### What if I'm not sure about the solution?

Write what you know.
State the options you see and which one you lean toward, and use the "Risks and
Open Questions" section to flag uncertainty.
A draft with acknowledged unknowns is more useful than no document at all.

If you need to experiment first, spike with code and write the RFD once you have
a clearer picture.

### How detailed should the design section be?

Detailed enough for a reviewer to evaluate the approach without reading
implementation code, but not so detailed that it becomes an implementation spec.
RFDs describe the "what" and "why" at an architectural level; the code is the
"how" at an implementation level.

The existing architecture documents show the expected level of detail: design
goals in tables, data flow descriptions, component responsibilities, and
migration paths.

### Can I update an accepted RFD?

Yes.
For small corrections (typos, clarifications, minor adjustments discovered
during implementation), edit the document directly.
For changes that alter the fundamental approach, write a new RFD.

### What about the existing architecture documents?

They remain in `docs/architecture/`.
Architecture documents describe the current system, while RFDs capture proposals
and decisions.
See [Relationship to Architecture
Documents](#relationship-to-architecture-documents).

### Do I need approval to merge an RFD?

Follow the project's normal pull request process.
At least one contributor with relevant context should review an RFD before it
merges.
The goal is consensus, not formal sign-off.

### What if my document doesn't fit a template?

Use whatever structure fits the content.
Templates are starting points, not required formats, and policy documents,
values statements, and process guidelines have their own natural shape.
Only the metadata header (Status, Category, Authors, Date) is required for the
lifecycle tooling.
See [RFD 002] for an example of a free-form RFD.

-----

## Implementation Plan

This RFD is its own implementation.
The steps are:

1. Create the `docs/rfd/` directory.
2. Add `000-design-template.md`, `000-decision-template.md`,
   `000-guide-template.md`, and `000-process-template.md`.
3. Provide the `jp rfd` lifecycle commands ([RFD 116]) to draft, promote,
   relate, supersede, abandon, and track RFDs, along with `just rfd-promote` in
   the `justfile`.
4. Add this document as `001-jp-rfd-process.md`.
5. Merge via pull request after discussion.
6. Follow this process for future RFDs.

[RFC 3]: https://datatracker.ietf.org/doc/html/rfc3
[RFD 002]: 002-using-llms.md
[RFD 116]: 116-shareable-rfd-ticket-and-site-tooling.md
[`000-decision-template.md`]: 000-decision-template.md
[`000-design-template.md`]: 000-design-template.md
[`000-guide-template.md`]: 000-guide-template.md
[`000-process-template.md`]: 000-process-template.md
[priority board]: priority.md
[ticket board]: ../ticket/board.md
[tickets]: 100-in-repo-ticket-tracking.md
