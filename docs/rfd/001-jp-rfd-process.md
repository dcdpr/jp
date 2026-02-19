# RFD 001: The JP RFD Process

- **Status**: Draft
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-17

## Summary

This document establishes the Request for Discussion (RFD) process for the JP
project. RFDs are short design documents that describe a significant change — a
new feature, an architectural shift, a process change — before implementation
begins. The goal is to think clearly, communicate intent, and invite feedback
early.

## Motivation

JP already has a `docs/architecture` directory with documents that describe
system designs in detail. Several of these (query stream pipeline, structured
output, stateful tool inquiries, wasm tools) are effectively design proposals:
they describe what we intend to build, why, and how. But they lack a formal
lifecycle — there is no way to distinguish a proposal under discussion from an
accepted design, or to track when a document was superseded.

We want a lightweight process that:

1. Gives design documents a clear lifecycle (draft → discussion → accepted →
   implemented; or abandoned / superseded).
2. Lowers the barrier to proposing ideas — rough thoughts are welcome.
3. Creates a searchable record of decisions and their rationale.
4. Works naturally with our existing Git + pull request workflow.

We do not want a process that adds bureaucracy, requires approvals from
committees, or discourages people from writing things down.

## Principles

The RFD process is guided by a few core beliefs, drawn from the IETF's original
RFC spirit and refined for a small, fast-moving open-source project.

### Timely over polished

A rough document written now is more valuable than a perfect document written
never. RFDs are encouraged to be concise and direct. An RFD can be a single
page. Grammar and formatting matter less than clarity of thought.

> "Notes are encouraged to be timely rather than polished. Philosophical
> positions without examples or other specifics, specific suggestions or
> implementation techniques without introductory or background explication, and
> explicit questions without any attempted answers are all acceptable. The
> minimum length for a note is one sentence."
>
> — [RFC 3](https://datatracker.ietf.org/doc/html/rfc3), Steve Crocker, 1969

### Opinionated with options

An RFD should propose a specific solution, not present an open-ended menu of
choices. The author's job is to navigate the problem space, evaluate
alternatives, and land on a recommendation. Readers should understand *what* is
proposed, *why* it was chosen, and *what else* was considered.

Ambiguity creates unproductive discussion. If you're unsure about the solution,
that's fine — state what you know, what you don't, and what you recommend given
current information. Spike with code if you need to build confidence.

### Small scope

Keep RFDs focused. One document, one topic. If a change has multiple independent
parts, write multiple RFDs. A focused document is easier to review, easier to
discuss, and leads to faster consensus.

Use the "Non-Goals" or "Future Work" sections to acknowledge related concerns
you're deliberately deferring. This signals awareness without bloating the
current proposal.

### Permanent record

RFDs are never deleted. If an idea is abandoned, the document is marked as such
with a brief explanation. If a design is superseded, the old document links to
the new one. This preserves the reasoning behind past decisions and helps future
contributors understand why things are the way they are.

## When to Write an RFD

Write an RFD when:

- Adding a new feature that affects the architecture or public interface
- Making a significant change to the data model or event system
- Introducing a new dependency, protocol, or integration pattern
- Changing the build, release, or contribution process
- Removing a feature or deprecating an interface
- Proposing a large refactoring effort
- Any change where you want structured feedback before investing in code

Do NOT write an RFD for:

- Bug fixes
- Performance improvements with no architectural change
- Code reorganization that doesn't change behavior
- Small feature additions that fit within established patterns
- Documentation updates

When in doubt, start writing. If it turns out to be unnecessary, you'll know
quickly. The cost of an unnecessary RFD is low; the cost of a misaligned
implementation is high.

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

The author is actively writing the document. It may be incomplete, have open
questions, or change significantly. Drafts live on a branch and are not yet
ready for formal review, but early feedback from collaborators is encouraged.

### Discussion

The RFD is complete enough to review. A pull request is opened to merge the
document into `main`. Discussion happens on the pull request. The author
incorporates feedback and iterates on the document.

There is no fixed timeline for discussion. For most RFDs, a few days should
suffice. If no one has reviewed your RFD after 48 hours, ask someone directly.
If discussion stalls, a synchronous conversation (call, chat) can help break the
deadlock.

### Accepted

Discussion has converged and the pull request is merged. The RFD represents the
agreed-upon direction. Implementation can begin.

An accepted RFD is not immutable. If implementation reveals issues, update the
document. For minor corrections, edit in place. For significant changes, write a
new RFD that supersedes the original.

### Implemented

The feature or change described in the RFD has been fully implemented. This is a
bookkeeping state — it signals that the document describes the current system,
not just a plan.

### Superseded

The design in this RFD has been replaced by a newer RFD. The original document
remains in the repository as a historical record. Its metadata is updated with a
**Superseded by** link pointing to the replacement, and the new RFD carries a
**Supersedes** link pointing back.

Superseded is distinct from Abandoned: a superseded RFD was accepted and may
have been partially or fully implemented, but a later design replaced it. An
abandoned RFD was never accepted or implemented.

An RFD can be superseded from either the Accepted or Implemented state.

### Abandoned

The idea was considered and deliberately set aside. The document remains in the
repository with a brief note explaining why. Common reasons: the problem was
solved differently, priorities changed, or the approach turned out to be
infeasible.

## Document Format

### Filename

```
docs/rfd/NNN-short-title.md
```

- `NNN` is a zero-padded, monotonically increasing serial number (001, 002, ...,
  010, ..., 100).
- `short-title` is a lowercase, hyphen-separated slug. Keep it short but
  descriptive.
- Numbers are never reused. If an RFD is abandoned, its number is retired.

### Templates

RFDs come in two styles, each with its own template:

| Style   | Template                | Use when                                 |
|---------|-------------------------|------------------------------------------|
| **rfd** | [`000-rfd-template.md`] | Proposing a feature, architectural       |
|         |                         | change, or process change that needs a   |
|         |                         | design.                                  |
| **adr** | [`000-adr-template.md`] | Recording a decision that doesn't need a |
|         |                         | full design — e.g., a technology choice, |
|         |                         | a convention, a policy.                  |

Both styles share the same numbering scheme, directory, lifecycle, and review
process. The difference is scope: an RFD-style document has a full design
section and implementation plan; an ADR-style document has a concise context,
decision, and consequences.

**The templates are starting points, not constraints.** Some documents don't fit
either template — a policy statement, a values document, a process guideline.
That's fine. Structure the document however it reads best. The only requirement
is the metadata header (Status, Authors, Date) so the tooling and lifecycle
work. Delete the template sections that don't apply, add sections that do, or
write something entirely free-form.

To create a new draft:

```sh
just rfd-draft rfd "My Feature Title"    # design proposal
just rfd-draft adr "Use TOML for Config" # decision record
```

This copies the appropriate template to the next available number (e.g.
`docs/rfd/002-my-feature-title.md`), fills in the title, author, and today's
date, and sets the status to **Draft**.

[`000-rfd-template.md`]: 000-rfd-template.md
[`000-adr-template.md`]: 000-adr-template.md

#### RFD sections

Not all sections are required for every RFD — omit sections that genuinely don't
apply — but think twice before skipping one. Every section can be brief. A
one-sentence Alternatives section is better than no Alternatives section.

| Section                      | Purpose                                  |
|------------------------------|------------------------------------------|
| **Summary**                  | One to three sentences. A reader should  |
|                              | get the gist without reading further.    |
| **Motivation**               | Why is this change needed? What happens  |
|                              | if we do nothing? Start with "why"       |
|                              | before "how."                            |
| **Design**                   | The core proposal. Start with what the   |
|                              | user or caller sees before describing    |
|                              | internals. Structure freely — common     |
|                              | subsections: Overview, Design Goals,     |
|                              | Architecture, Data Flow, API Changes,    |
|                              | Configuration Changes.                   |
| **Drawbacks**                | Known costs of this approach. Argue      |
|                              | honestly against your own proposal.      |
| **Alternatives**             | What else was considered and why it was  |
|                              | rejected.                                |
| **Non-Goals**                | What this RFD explicitly does not aim to |
|                              | achieve, even though a reader might      |
|                              | expect it to.                            |
| **Risks and Open Questions** | What could go wrong? What don't we know  |
|                              | yet?                                     |
| **Implementation Plan**      | Phases or steps. For each: what it       |
|                              | includes, what it depends on, whether it |
|                              | can be merged independently.             |
| **References**               | Links to related RFDs, issues,           |
|                              | documentation, or external resources.    |

#### ADR sections

ADRs are intentionally minimal. Four sections plus optional references:

| Section                      | Purpose                                  |
|------------------------------|------------------------------------------|
| **Summary**                  | One to three sentences stating the       |
|                              | decision.                                |
| **Context**                  | The forces at play: constraints, needs,  |
|                              | existing patterns. Facts, not opinions.  |
| **Decision**                 | The choice, stated clearly. "We will use |
|                              | X."                                      |
| **Consequences**             | What follows — positive, negative, and   |
|                              | neutral.                                 |
| **References**               | Links to related RFDs, issues, or        |
|                              | external resources.                      |

#### Metadata header

Both styles use the same metadata:

```markdown
- **Status**: Draft | Discussion | Accepted | Implemented | Superseded | Abandoned
- **Authors**: Name <email> (or GitHub handle)
- **Date**: YYYY-MM-DD
- **Supersedes**: RFD NNN (if applicable)
- **Superseded by**: RFD NNN (if applicable)
```

### Writing Style

- **Use present tense.** "This RFD describes..." not "This RFD was created to
  describe..."
- **Be direct.** Avoid hedging language like "it seems" or "probably" or "it
  might be worth considering." State what you propose and why.
- **Use concrete examples.** A code snippet or data flow diagram is worth a
  paragraph of abstract description.
- **Define terms.** If you introduce a concept, define it where it first
  appears.
- **Keep it short.** If an RFD exceeds 5-6 pages (roughly 2000 words), consider
  whether it can be split into smaller proposals.

## Process

### Creating an RFD

1. Create a branch for your work.
2. Run `just rfd-draft rfd Your Title` (or `just rfd-draft adr Your Title` for a
   decision record) to generate the file from the appropriate template.
3. Fill in the sections. Write your proposal.
4. Push your branch. Iterate until you're ready for feedback.

### Opening for Discussion

1. Run `just rfd-promote NNN` to advance the status to **Discussion**.
2. Open a pull request to merge your branch into `main`.
3. Tag reviewers — people with context on the problem area.
4. Engage with feedback. Update the document as the discussion evolves.

### Accepting an RFD

1. When discussion converges, run `just rfd-promote NNN` to advance the
   status to **Accepted**.
2. Merge the pull request.
3. Create implementation issues or tasks as needed.

### After Acceptance

- **Minor updates**: Edit the document directly on `main` via a standard pull
  request. No new RFD number needed.
- **Significant changes**: Write a new RFD that supersedes the original.
- **Implementation complete**: Run `just rfd-promote NNN` to advance the
  status to **Implemented**.
- **Design superseded**: Write a new RFD, then run
  `just rfd-supersede NNN MMM` to mark the old RFD as superseded and
  cross-link both documents.
- **Idea abandoned**: Run `just rfd-abandon NNN "reason"` to mark the
  RFD as abandoned with an explanation.

### Tooling

All RFD commands are in the `docs` group. Run `just --list --group docs`
to see them.

| Command | Description |
|---|---|
| `just rfd-draft rfd\|adr TITLE` | Create a new RFD from the appropriate template. |
| `just rfd-promote NNN` | Advance status: Draft → Discussion → Accepted → Implemented. |
| `just rfd-supersede NNN MMM` | Mark RFD NNN as superseded by RFD MMM, updating both. |
| `just rfd-abandon NNN REASON` | Mark RFD NNN as abandoned with the given reason. |
| `just rfd-grep TERM` | Search across all RFD documents using `rg`. |

## Relationship to Architecture Documents

The existing `docs/architecture/` directory contains detailed technical
descriptions of implemented systems. These serve a different purpose than RFDs:

|               | RFDs (`docs/rfd/`)                  | Architecture Docs (`docs/architecture/`) |
|---------------|-------------------------------------|------------------------------------------|
| **Purpose**   | Propose a change                    | Describe the current system              |
| **Lifecycle** | Draft → Accepted → Implemented      | Living documents, updated as the system  |
|               |                                     | evolves                                  |
| **Audience**  | Contributors deciding what to build | Contributors understanding what exists   |
| **Scope**     | A specific change or feature        | A subsystem or cross-cutting concern     |

The typical flow: an RFD proposes a design, gets accepted, and is implemented.
Once implemented, the relevant architecture documents are updated to reflect the
new state of the system. The RFD remains as a historical record of the decision.

Over time, some existing architecture documents may be retroactively referenced
by RFDs, or new architecture documents may be created as companions to accepted
RFDs. The two directories complement each other.

## FAQ

### What if I'm not sure about the solution?

Write what you know. State the options you see and which one you lean toward.
Use the "Risks and Open Questions" section to flag uncertainty. A draft with
acknowledged unknowns is more useful than no document at all.

If you need to experiment first, do that. Write the RFD after you've spiked and
have a clearer picture.

### How detailed should the design section be?

Detailed enough that a reviewer can evaluate the approach without reading the
implementation code. Not so detailed that it becomes the implementation spec.
RFDs describe the "what" and "why" at an architectural level; the code is the
"how" at an implementation level.

For JP specifically, the existing architecture documents provide a good
reference for the level of detail expected: design goals as tables, data flow
descriptions, component responsibilities, migration paths.

### Can I update an accepted RFD?

Yes. For small corrections (typos, clarifications, minor adjustments discovered
during implementation), edit the document directly. For changes that alter the
fundamental approach, write a new RFD.

### What about the existing architecture documents?

They stay where they are. The architecture directory describes the system as it
is. The RFD directory captures proposals and decisions. Both are valuable. See
[Relationship to Architecture Documents](#relationship-to-architecture-documents).

### Do I need approval to merge an RFD?

Follow the project's normal pull request process. An RFD should be reviewed by
at least one other contributor with relevant context before merging. The goal is
consensus, not a formal sign-off process.

### What if my document doesn't fit either template?

Use whatever structure makes sense. The templates are suggestions to help you
get started, not a format you must follow. Policy documents, values statements,
process guidelines — these have their own natural shape. The only hard
requirement is the metadata header (Status, Authors, Date) at the top of the
file, so the lifecycle tooling works. See [RFD 002](002-using-llms.md) for an
example of a free-form RFD.

---

## Implementation Plan

This RFD is its own implementation. The steps are:

1. Create the `docs/rfd/` directory.
2. Add `000-rfd-template.md` for design proposals and `000-adr-template.md` for
   decision records.
3. Add `just` tasks for the RFD lifecycle: `rfd-draft`, `rfd-promote`,
   `rfd-supersede`, `rfd-abandon`, and `rfd-grep`.
4. Add this document as `001-jp-rfd-process.md`.
5. Merge via pull request after discussion.
6. Future RFDs follow the process described here.
