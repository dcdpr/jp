# RFD 099: RFD Authoring and Review Pipeline

- **Status**: Discussion
- **Category**: Process
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-07
- **Extends**: [RFD 003]

## Summary

This RFD describes how JP writes and reviews RFDs.
Authoring splits into a scope gate the contributor approves and a draft written
against it.
Review then runs unattended under a shrinking finding budget, and the
contributor's signoff is the terminal stage.

## Motivation

RFDs pay for themselves at implementation time.
Getting one to that point costs many rounds of back-and-forth that produce a
longer document rather than a better one.

Three mechanisms cause this, and they compound.

**The reviewer has no terminal state.** Nothing defines "done", so a reviewer
pointed at a document that changed since the last round always finds something.
The contributor becomes the stopping condition.

**The severity framework classifies findings, then discards the
classification.** It defines a gating finding as one where a plausible input
produces damage that spreads or hides.
It names the contained-and-visible case as where review threads go to die.
It then instructs the reviewer to post everything anyway.

**Every finding resolves by adding text.** A triager that declines a finding
still writes a sentence acknowledging it, so the document only grows.
RFD 070 reached 11,671 words against a 2,000-word guideline.

## The Pipeline

Five stages, each with one owner, and one loop back.
Each stage gets only the write access it needs.
The outline stage writes nothing.
The applier writes the RFD, and files a ticket at signoff.

### Outline

The contributor discusses the problem until the design is settled, then runs
`just rfd-this`.
That forks the conversation before applying the author configuration, so the
design conversation is a branch rather than a change to the one you were in.
The author reads that discussion and emits an outline: category, summary, the
one problem, the proposed shape, alternatives, Non-Goals, and a per-section word
budget.
No file is written.

The contributor approves, cuts, or redirects.
This is the scope gate.
Removing a section here costs one line.
Removing it after review has attached findings to it costs an argument.

### Draft

`just rfd-write` writes the file to the approved outline.
A section that runs over budget takes the overage from another section or gets
cut.
It does not raise the total.

### Cycle

`just rfd-cycle NNN [ROUNDS]` runs review, triage, apply, and lint without
prompting, committing each round separately so the run reads as a diff.
It stops when the reviewer returns no findings, a stage escalates, or the round
budget runs out.

Deciding and editing stay in separate turns.
The triager rules on each finding and describes the edit.
The applier makes only the edits ruled `accept` or `amend`.
An assistant that both decides and edits reasons its way into the
acknowledgement sentences this pipeline exists to remove.

The cycle records worktree changes outside the target RFD for signoff without
attributing them to the applier.

### Signoff

`just rfd-signoff NNN` opens the RFD in `revdiff` beside a ledger of what the
cycle raised and dropped.
Notes on either file go to the applier.
A note on a deferred ledger line files the ticket the triage named, so the item
leaves the cycle without entering the document.
Leaving no notes is the verdict.

### Check-in

Some notes are questions, not instructions.
The applier lists those instead of answering them, and signoff routes them to
`just rfd-checkin`.

That returns to the conversation that designed the RFD, carrying the document,
the ledger, and the signoff round.
Only that conversation holds the rejected options and the constraints found
along the way, and an open question usually turns on exactly that.
Settle it there, update the RFD, run the cycle again.

This is the only backwards edge in the pipeline.
It exists because acceptance keeps discovering that a decision was never made.

### Promote

`just rfd-promote NNN` advances the status.
The pipeline runs `just rfd-lint` before the contributor invokes it.
A hand-written RFD can use the same command as an advisory check.

## Budgets and Writing Rules

Every assistant-authored RFD has a prose budget in words, with fenced code
blocks and the metadata header excluded.

| Category | Target | Hard limit |
| -------- | ------ | ---------- |
| Design   | 1200   | 2000       |
| Decision | 500    | 800        |
| Guide    | 2000   | 3500       |
| Process  | 2000   | 3500       |

Write to the target.
The gap to the hard limit belongs to the review cycle, which will surface things
that genuinely need saying.
An RFD that opens at the hard limit has nowhere to put them.

The hard limit gates pipeline readiness.
The cycle cannot declare an assistant-authored RFD ready while `just rfd-lint`
reports a budget error.

Four sentence-level rules come from ASD-STE100, the controlled English used in
aircraft maintenance manuals.
Aim for 25 words per sentence.
`just rfd-lint` warns above 30, leaving a small tolerance for prose that reads
worse when split.
One word keeps one meaning across the document.
`can`, `will`, and `must` replace `should`, `would`, `may`, and `might`.
Voice is active and tenses are simple.

The full standard is not adopted.
It bans the argumentative prose that Motivation and Alternatives are made of.

`just rfd-lint` is authoritative for the above.
It also checks link hygiene, metadata, filename and heading agreement, and
duplicated sentences.
Errors gate promotion and CI.
Line-level findings are warnings and never gate, because a long sentence written
months ago must not block acceptance today.

## Review Protocol

Review has a terminal state, and reaching it is the job.
A review that produces findings forever has failed, not succeeded.

### The finding bar

A finding exists when a plausible input produces damage that spreads or hides.
Damage spreads when it escapes the operation under review.
Damage hides when the user cannot tell it happened.

Anything contained and visible is an observation.
Observations go under a `Noted` heading, and the triager must not act on them.
They are on the record, which was the point.

### Non-Goals bind the reviewer

The Non-Goals section is the scope contract between author and reviewer.
A reviewer must not raise a finding that falls inside a stated Non-Goal.
A reviewer can challenge a Non-Goal itself once, in round 1.
After that, scope is settled.

That makes the section carry weight.
Name what is out of scope, and say whether it is deferred or rejected.

### Rounds

One round is review, then triage, then apply, then lint.
The finding budget shrinks each round: seven, then four, then two, then none.

After round 1, a new finding needs a reason to be new.
An edit introduced it, resolving an earlier finding exposed it, or new evidence
made an earlier conclusion false.
A concern present in round 1 that went unraised is settled by omission.
Progress is monotonic.

### Structured responses

Each step answers under a schema, and the orchestrator reads typed fields.
Every schema carries a prose `conclusion`, which is the part printed to the
terminal.

If the applier returns without changing the RFD, the cycle asks it to apply the
edits once more before carrying them to signoff.

State is derived from the data, never asserted alongside it.
An empty `findings` array is how a review clears an RFD, so there is no verdict
field that can contradict the findings it summarises.
An empty `needs_author` array is how a signoff reports nothing outstanding.
Tallies are counted from the rulings, and word counts come from `rfd-lint`.

### Rulings

The triager rules `accept`, `amend`, `decline`, `dismiss`, `defer`, or
`escalate` on each finding.

A `decline` produces no edit to the RFD.
Not a sentence in Risks, not a parenthetical, not a Non-Goal bullet.
The triage conversation is the record, and it is durable and searchable.
Storing rejected edge cases in the document is what turns a 1,200-word RFD into
a 5,000-word one, one reasonable-looking sentence at a time.

### Escalation

Two disagreements on the same item rule it `escalate`, which removes it from the
cycle.
An accept the applier cannot apply escalates the same way: a decision nobody has
made is not an edit.
Escalation is the only thing that interrupts an unattended run, and it always
means human judgment is required.

## Other Entry Points

The five stages are the default path.
Three others exist.

`just rfd-prose NNN` cuts words without changing content, in a single pass.
It stays separate from review, because "is the design right" and "is the prose
tight" are different questions that fight when asked in one conversation.
Run it once, when the design has converged.

`just rfd-review NNN` and `just rfd-triage NNN` drive one round by hand, with
the same personas the cycle uses.
Useful when a round wants watching.

`just rfd-draft CATEGORY TITLE` creates a blank draft from a template, for an
RFD written without the pipeline.
Everything after the draft still applies to it.

The `rfd` skill is the other assistant path.
Loaded into any conversation, it makes JP a collaborator on a document you are
working through rather than its author.
[RFD 003] covers when each is appropriate and who is responsible for the result.

## Non-Goals

- **Model-based checks in CI.** Rejected.
  A nondeterministic check is a review even when it emits lint-shaped
  diagnostics, and calling it a lint hides the distinction.
- **Filing a deferred finding automatically during a cycle.** Rejected.
  A ticket is a committed file, and the per-round commit is scoped to the RFD,
  so one filed mid-cycle sits uncommitted across every round after it.
  Deferrals reach the carry-over ledger, and signoff files them where the author
  sees the diff.
- **Automatic promotion after a clean signoff.** Rejected.
  Promotion is a one-way door and stays a deliberate act.
- **The RFD lifecycle.** Rejected.
  States, numbering, relationship metadata, and templates stay in [RFD 001].

## Alternatives

**Cap the existing interactive loop at N rounds.** This caps the symptom.
A reviewer with no definition of done spends every one of the N rounds finding
something.

**Merge triage and editing into one model turn.** Fewer round trips, at the cost
of the separation that stops a declined finding from becoming a hedging
sentence.
The extra model call buys that guarantee.

**Parse control state from model prose.** Footer tokens keep responses
free-form, but the prose and token can contradict each other.
Structured arrays carry the primary data, while their `conclusion` fields retain
human-readable prose.

**Adopt ASD-STE100 in full.** It bans the modal and hypothetical constructions
that design rationale needs.
Four of its rules transfer; the rest would make Motivation and Alternatives
worse.

**Put the contributor's review before the cycle.** This reintroduces the
per-round human involvement the pipeline removes, and the outline gate already
settles scope before prose exists.

## Implementation Plan

1. Land the schemas, personas, `rfd-lint`, and `just` recipes.
2. Trial the cycle on in-flight RFDs, and tune the finding budgets from the
   triage tallies.
3. Keep [RFD 001], [RFD 002], and [RFD 003] aligned with the pipeline's scope
   and the contributor's responsibility.

[RFD 001]: 001-jp-rfd-process.md
[RFD 002]: 002-using-llms.md
[RFD 003]: 003-jp-assisted-rfds.md
