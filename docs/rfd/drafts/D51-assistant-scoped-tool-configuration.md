<!--
  This template is a starting point, not a constraint. Delete sections that
  don't apply, add sections that do, or restructure entirely. The only
  requirement is the metadata header (Status, Authors, Date).

  Use HTML comments like this one for draft-time notes and review markers.
  They do not appear in the rendered output and can be removed when the RFD
  advances to Discussion status.
-->

# RFD D51: Assistant-Scoped Tool Configuration

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-07
- **Required by**: [RFD D54]

## Summary

JP's per-conversation tool bindings live under `conversation.tools.*`, but they
are de facto scoped to the single assistant that consumes them.
This RFD moves tool configuration to an assistant-scoped `assistant.tools.*`,
keeping `conversation.tools.*` accepted as legacy input for backward
compatibility.
It covers the schema change, the compatibility mapping, the negative-delta
claims-map field paths, and which documents are updated versus preserved as a
historical record.

## Motivation

`conversation.tools.<name>` configures which tools an assistant may call and how
— source, run policy, access, display, and options.
Today a conversation has exactly one assistant, so "the conversation's tools"
and "the assistant's tools" are the same set.
The key places the configuration on the conversation, but every consumer —
query setup, tool rendering, enable and disable flags, and the tool coordinator
— reads it as the assistant's tools.

This gap between where the configuration lives and who owns it is a small but
real source of confusion, and it blocks any future design in which one
conversation hosts more than one assistant.
Making the scope explicit, with `assistant.tools.*` as the canonical location,
names the real owner without changing behavior for existing single-assistant
conversations.

Doing nothing keeps the mismatch and makes the eventual move more expensive.
`conversation.tools` is referenced across roughly twenty accepted RFDs, the
living configuration documentation, and the negative-delta claims map (RFD 070),
which enumerates `conversation.tools.*` field paths directly.
The longer the key remains, the more surfaces bind to it.
This RFD scopes the migration so it happens once, deliberately, with a defined
compatibility window for the legacy key.

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

[RFD D54]: D54-multi-participant-conversations.md
