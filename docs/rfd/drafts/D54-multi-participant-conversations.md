<!--
  This template is a starting point, not a constraint. Delete sections that
  don't apply, add sections that do, or restructure entirely. The only
  requirement is the metadata header (Status, Authors, Date).

  Use HTML comments like this one for draft-time notes and review markers.
  They do not appear in the rendered output and can be removed when the RFD
  advances to Discussion status.
-->

# RFD D54: Multi-Participant Conversations

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-07
- **Requires**: [RFD D24], [RFD 070], [RFD D51], [RFD D52], [RFD D53]

## Summary

This RFD generalizes JP conversations from one human and one assistant to many
participants sharing one event stream: several named assistant participants and
the human deliberating in one room.
The detailed design is deferred until its prerequisites are accepted; this
document records the goal, the motivation, and the dependency chain.

## Motivation

Several JP workflows need more than one assistant voice in one shared context.
A pull-request panel is the motivating example: a reviewer assistant, a triager
assistant, and the human deliberating in one room, rather than maintaining
separate conversations and relaying context between them by hand.

An earlier end-to-end exploration of this design surfaced two conclusions.
First, multi-participant conversations rest on several capabilities that are
valuable on their own and should land first: assistant-scoped tool
configuration, explicit request-to-response event linking, inline attachment URI
parsing (which frees the `@` prefix this design needs for addressing), stable
event identifiers (RFD D24), and negative config deltas (RFD 070).
Each is independently useful to single-assistant JP and is being written as its
own RFD.
Second, once those exist — negative deltas in particular — a blank-sheet
design is likely to be materially simpler than one layered onto today's
config-delta model, which is why this RFD does not carry forward the earlier
draft's design.

This RFD therefore defers its own design until its prerequisites are accepted.
It exists now to record the goal and to hold the dependency edges in its
metadata header, so the sequencing is explicit and the design begins from the
right foundation.

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

[RFD 070]: ../070-negative-config-deltas.md
[RFD D24]: D24-stable-event-identifiers.md
[RFD D51]: D51-assistant-scoped-tool-configuration.md
[RFD D52]: D52-request-response-event-linking.md
[RFD D53]: D53-inline-attachment-uri-parsing.md
