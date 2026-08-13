# RFD 003: JP-Assisted RFD Writing

- **Status**: Implemented
- **Category**: Process
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-17
- **Extended by**: [RFD 099]

## Summary

This RFD sets the terms on which an assistant writes an RFD for this project.
The contributor owns the problem, the decisions, and the approval; the assistant
can own the prose.
It also says why an assistant-authored RFD carries constraints a hand-written
one does not.

## Motivation

Writing an RFD means understanding the project's conventions ([RFD 001]),
reading the code that informs the design, and structuring a proposal that argues
for itself.
JP is good at all three, and almost every RFD in this repository is
assistant-authored.

That is a fine arrangement, and it has one failure mode worth naming precisely.

## Delegating the Typing, Not the Thinking

The contributor settles the problem and the design in conversation.
The assistant then writes the document from those decisions.
The contributor reads the result and confirms it says what was agreed.

The failure is not generated prose.
It is generated prose standing in for a decision nobody made.
It shows up in four shapes:

- **An unsettled question, written up as settled.** Fluent prose about an open
  problem reads exactly like prose about a closed one.
- **Manufactured agreement.** A model will write "we chose X because Y" for an X
  nobody chose.
- **Invented scope.** A section appears because the template has a heading for
  it, not because the design needs it.
- **Length standing in for rigour.** A long document is not evidence that anyone
  resolved anything.

The countermeasure to the first three is the contributor: settle the scope
before asking for prose, then read what comes back against what was discussed.
The countermeasure to the fourth is mechanical, and it is the next section.

## Why Assistant-Authored RFDs Carry Extra Constraints

A human writing an RFD self-regulates on length.
They do not want to read a 10,000-word document, so they do not write one, and a
reviewer who receives one will send it back before reading it.

An assistant has neither instinct.
It writes at whatever length the prompt implies, and it will keep adding while
anything is still asking to be addressed.
Given a review loop with no terminal state, it produces a document that grows
every round: each addition reasonable, the total absurd.

So assistant-authored RFDs are held to numeric prose budgets, deterministic
checks that gate promotion, and a review protocol with a bounded number of
rounds.
Those constraints compensate for a missing instinct.
They are not a general standard for RFD quality, and a hand-written RFD is not
subject to them.

[RFD 099] documents the pipeline that enforces this.

## Responsibility

You own the content of an RFD whatever tools you used to write it.

That is the whole rule, and it does not soften because a model produced the
words.
A reviewer's time is spent on the assumption that the author understands the
proposal.
An RFD whose author cannot defend a section did not have an authoring problem;
it had a design problem, and the prose hid it.

Automated review does not transfer any of this.
Two models clearing a document means they agree, which is not the same as the
document being right.
The contributor's approval is the only signal that carries weight, and it is
given by reading the final result.

## Relationship to RFD 002

[RFD 002] covers LLM use across the project and assigns responsibility for a
generated artifact to whoever approves it.
This RFD applies that to RFDs specifically.

RFD 002's "LLMs as editors" and "LLMs as researchers" patterns need no
qualification here.
Its "LLMs as writers" pattern applies under the condition it sets: the
contributor supplies the decisions and approves the result.

## Non-Goals

- **Design generation from a one-line prompt.** Rejected.
  The assistant writes after a discussion settles the problem, not instead of
  one.
  A title is not a design.
- **Model review as a CI check.** Rejected.
  A nondeterministic check is a review even when it emits lint-shaped output,
  and calling it a lint hides the distinction that matters.
- **Documenting the skill and pipeline configuration here.** Rejected.
  Which tools a skill enables and which files it attaches are properties of the
  configuration, which documents itself and drifts the moment prose describes
  it.

## References

- [RFD 001] for the RFD process itself, which applies to every RFD however it
  was written.
- [RFD 002] for LLM use across the project.
- [RFD 099] for the authoring and review pipeline.

[RFD 001]: 001-jp-rfd-process.md
[RFD 002]: 002-using-llms.md
[RFD 099]: 099-rfd-authoring-and-review-pipeline.md
