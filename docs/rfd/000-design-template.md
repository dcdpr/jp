<!--
  This template is a starting point, not a constraint. Delete sections that
  don't apply, add sections that do, or restructure entirely. The only
  requirement is the metadata header (Status, Authors, Date).

  Budget: 1200 prose words, 2000 hard. Write to the target; the gap is for
  what review turns up. Run `just rfd-lint DNN` to check. A Design RFD that
  can't make its case in 1200 words is usually two RFDs.

  Non-Goals is not optional here. It is the binding scope contract: a
  reviewer may not raise a finding inside a stated Non-Goal.

  Use HTML comments like this one for draft-time notes and review markers.
  They do not appear in the rendered output and must be removed before the
  RFD advances to Discussion status.
-->

# RFD NNN: TITLE

- **Status**: Draft
- **Category**: Design
- **Authors**: AUTHOR
- **Date**: DATE

## Summary

One to three sentences describing what this RFD proposes.
A reader should be able to understand the gist without reading further.

## Motivation

Why is this change needed?
What problem does it solve?
What happens if we do nothing?
Start with "why" before "how."

## Design

The core of the RFD.
Describe the proposed solution in enough detail that someone familiar with the
codebase could implement it.
Use diagrams, code snippets, and examples where they help.

Start with what the user or caller sees — the external behavior, API, or
experience — before describing internals.

Structure this section however makes sense for the topic.
Common subsections include: Overview, Design Goals, Architecture, Data Flow, API
Changes, Configuration Changes.

Every section can be brief.
A one-sentence Alternatives section is better than no Alternatives section.

## Drawbacks

What are the known costs of this approach?
What does the project give up by adopting it?
Argue honestly against your own proposal.

## Alternatives

What other approaches were considered?
Why were they rejected?
This section is important — it shows the reader that the solution space was
explored and gives future readers context for the decision.

## Non-Goals

What this RFD explicitly does not aim to achieve, even though a reader might
expect it to.

This section binds the review: a reviewer cannot raise a finding that falls
inside a stated Non-Goal, and gets one chance to challenge a Non-Goal itself.
Name each item in the project's vocabulary, and say whether it is deferred
(someone does it later) or rejected (nobody should).

Do not use this section to dodge a question the design has to answer.
A Non-Goal that hides a hole in the design is worse than the hole.

## Risks and Open Questions

What could go wrong?
What don't we know yet?
What needs to be validated during implementation?
It's better to surface uncertainty explicitly than to pretend it doesn't exist.

## Implementation Plan

How will this be implemented?
Break the work into phases or steps.
This section bridges the gap between design and execution.

For each phase, briefly describe:

- What it includes
- What it depends on (other phases, or other RFDs by number)
- Whether it can be reviewed and merged independently

If a phase has measurable cost implications (token budget, latency, binary size,
API calls), include a brief quantitative estimate.

## References

Links to related RFDs, issues, documentation, or external resources.
