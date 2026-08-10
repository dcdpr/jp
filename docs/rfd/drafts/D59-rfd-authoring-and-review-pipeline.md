# RFD D59: RFD Authoring and Review Pipeline

- **Status**: Draft
- **Category**: Process
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-07
- **Extends**: [RFD 001]

## Summary

This RFD describes how JP writes and reviews RFDs.
Authoring splits into a scope gate the contributor approves and a draft written
against it.
Review then runs unattended under a shrinking finding budget, and the
contributor's signoff is the terminal stage.

## Motivation

RFDs pay for themselves at implementation time.
Getting one to that point costs hours of back-and-forth that produce a longer
document rather than a better one.

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

Five stages, each with one owner.
Each stage gets only the write access it needs: the outline stage gets none, and
the applier writes only the RFD.

### Outline

The contributor discusses the problem until the design is settled, then runs
`just rfd-this`.
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
It stops on the first of: the reviewer returning `CLEAR`, either side raising
`ESCALATE:`, or the round budget running out.

Deciding and editing stay in separate turns.
The triager rules on each finding and describes the edit.
The applier makes only the edits ruled `Accept` or `Amend`.
An agent that both decides and edits reasons its way into the acknowledgement
sentences this pipeline exists to remove.

The cycle snapshots every file except the RFD and stops if any of them changes.

### Signoff

`just rfd-signoff NNN` opens the RFD in `revdiff` beside a ledger of what the
cycle raised and dropped.
Notes on either file go to the applier.
A note on a deferred ledger line files the ticket the triage named, so the item
leaves the cycle without entering the document.
Leaving no notes is the verdict.

### Promote

`just rfd-promote NNN` advances the status.
It refuses to advance an RFD that `just rfd-lint` reports errors on.

## Budgets and Writing Rules

Every RFD has a prose budget in words, with fenced code blocks and the metadata
header excluded.

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

The hard limit gates promotion.
An author who needs the words passes a reason to `just rfd-promote`, which
records it as `- **Over budget**: <reason>` in the metadata header where every
reader sees it.

Four sentence-level rules come from ASD-STE100, the controlled English used in
aircraft maintenance manuals.
One sentence carries at most 25 words.
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

Every review ends with `VERDICT: CLEAR` or `VERDICT: BLOCK` on its own final
line.

### Verdicts

The triager rules `Accept`, `Amend`, `Decline`, `Dismiss`, `Defer`, or
`Escalate` on each finding.

A `Decline` produces no edit to the RFD.
Not a sentence in Risks, not a parenthetical, not a Non-Goal bullet.
The triage conversation is the record, and it is durable and searchable.
Storing rejected edge cases in the document is what turns a 1,200-word RFD into
a 5,000-word one, one reasonable-looking sentence at a time.

The triager reports the word delta of its edits before anything is applied.

### Escalation

Two disagreements on the same item mark it `ESCALATE:` and remove it from the
cycle.
Repeated disagreement is the signal that human judgment is required, and it is
the only thing that interrupts an unattended run.

## Non-Goals

- **Model-based checks in CI.** Rejected.
  A nondeterministic check is a review even when it emits lint-shaped
  diagnostics, and calling it a lint hides the distinction.
- **Filing a deferred finding automatically during a cycle.** Rejected.
  A ticket is a committed file, and the per-round commit is scoped to the RFD,
  so one filed mid-cycle sits uncommitted across every round after it.
  Deferrals reach the carry-over ledger, and signoff files them where the
  author sees the diff.
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

**Constrain every model turn with a JSON schema.** Machine-readable verdicts
replace a regex on one token.
The cost is the prose that the carry-over ledger and the signoff stage are built
from.

**Adopt ASD-STE100 in full.** It bans the modal and hypothetical constructions
that design rationale needs.
Four of its rules transfer; the rest would make Motivation and Alternatives
worse.

**Put the contributor's review before the cycle.** This reintroduces the
per-round human involvement the pipeline removes, and the outline gate already
settles scope before prose exists.

## Implementation Plan

1. Land the personas, `rfd-lint`, and the `just` recipes.
2. Trial the cycle on in-flight drafts, and tune the finding budgets from what
   the triage tallies show.
3. Promote this RFD, then move the pipeline sections out of [RFD 001] and
   cross-reference it.
   [RFD 001] cannot link here until this document leaves Draft.
4. Update [RFD 002] and [RFD 003], which describe the assistant as a
   collaborator that does not produce the finished document.

[RFD 001]: ../001-jp-rfd-process.md
[RFD 002]: ../002-using-llms.md
[RFD 003]: ../003-jp-assisted-rfds.md
