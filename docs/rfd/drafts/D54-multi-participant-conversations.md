# RFD D54: Multi-Participant Conversations

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-07
- **Requires**: [RFD 070], [RFD D51], [RFD 098], [RFD D53]
- **Summary**: Generalizes conversations from a single human-assistant pair to
  multiple named assistant and human participants sharing one event stream.

## Summary

This RFD generalizes JP conversations from a single human-assistant pair to
multiple named assistant participants and the human sharing one event stream.
The detailed design is deferred until its prerequisites are accepted; this
document records the goal, motivation, and dependency chain.

## Motivation

Several JP workflows need more than one assistant in a shared context.
A pull-request panel is the motivating example: a reviewer assistant, a triager
assistant, and the human can deliberate in a single conversation rather than
maintaining separate conversations and relaying context between them by hand.

An earlier end-to-end exploration of this design surfaced two conclusions.
First, multi-participant conversations depend on several capabilities that are
independently useful in single-assistant JP and warrant their own RFDs:
assistant-scoped tool configuration, explicit request-to-response event linking,
inline attachment URI parsing (which frees the `@` prefix for participant
addressing), stable event identifiers ([RFD 097]), and negative config deltas
(RFD 070).
Second, once these prerequisites exist (particularly negative config deltas), a
blank-sheet design is simpler than one layered onto the current config-delta
model, so this RFD does not carry forward the earlier draft's design.

This RFD therefore defers its detailed design until those prerequisites are
accepted.
Recording the goal and the dependency edges in the metadata header keeps the
sequencing explicit so the design begins from the right foundation.

## Design

Describe the proposed solution in enough detail that someone familiar with the
codebase can implement it.
Use diagrams, code snippets, and examples where they help.

Start with what the user or caller sees (the external behavior, API, or
experience) before describing internals.

Structure this section however makes sense for the topic.
Common subsections include Overview, Design Goals, Architecture, Data Flow, API
Changes, and Configuration Changes.

Every section can be brief.
A one-sentence Alternatives section is better than no Alternatives section.

## Drawbacks

What are the known costs of this approach?
What does the project give up by adopting it?
Argue honestly against your own proposal.

## Alternatives

What other approaches were considered, and why were they rejected?
Documenting rejected options shows that the solution space was explored and
preserves context for future readers.

## Non-Goals

State what this RFD explicitly does not aim to achieve, even where a reader
might expect it to.
Explicit non-goals keep the discussion focused and show how the proposal fits
the broader system.

## Risks and Open Questions

What could go wrong?
What don't we know yet?
What needs to be validated during implementation?
Surface uncertainty explicitly rather than leaving it implicit.

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

Link to related RFDs, issues, documentation, or external resources.

[RFD 070]: ../070-negative-config-deltas.md
[RFD 097]: ../097-stable-event-identifiers.md
[RFD 098]: ../098-request-response-event-linking.md
[RFD D51]: D51-assistant-scoped-tool-configuration.md
[RFD D53]: D53-inline-attachment-uri-parsing.md
