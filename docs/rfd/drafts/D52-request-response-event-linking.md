<!--
  This template is a starting point, not a constraint. Delete sections that
  don't apply, add sections that do, or restructure entirely. The only
  requirement is the metadata header (Status, Authors, Date).

  Use HTML comments like this one for draft-time notes and review markers.
  They do not appear in the rendered output and can be removed when the RFD
  advances to Discussion status.
-->

# RFD D52: Request-Response Event Linking

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-07
- **Required by**: [RFD D54]
- **Requires**: [RFD 097]

## Summary

Response events in a conversation stream carry no explicit reference to the
request they answer; the relationship is inferred from position within a turn.
This RFD adds an explicit link from each response event to the request event it
answers, keyed by the stable `event_id` introduced in RFD 097.
It defines which event pairs are linked and how a dangling link is resolved.

## Motivation

A conversation stream interleaves requests — chat requests from the human —
with the events that answer them: chat responses, plus the tool and inquiry
events produced while answering.
The binding between a response and its originating request is currently
positional: a reader assumes a response belongs to the most recent request in
the same turn.

Positional binding is fragile under exactly the hand edits JP encourages on
`events.json`.
Deleting a digression, rewinding an in-flight query, or dropping a noisy tool
call can silently change which request a response appears to answer.
Several proposed capabilities — branching, undo, compaction anchoring, and
faithful turn reconstruction — need to know unambiguously which request a given
response answers, and cannot rely on position surviving a structural edit.

RFD 097 gives every stream entry a stable `event_id` but deliberately leaves
reference semantics to its consumers.
This RFD is one such consumer: it records the response-to-request relationship
as an explicit `event_id` reference, so the link survives reordering and
deletion as a detectable reference rather than a positional guess.

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
This keeps the discussion focused and signals awareness of the broader picture.

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

[RFD 097]: ../097-stable-event-identifiers.md
[RFD D54]: D54-multi-participant-conversations.md
