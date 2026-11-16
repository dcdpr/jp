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
- **Summary**: Moves per-conversation tool configuration from
  `conversation.tools.*` to `assistant.tools.*` while retaining
  `conversation.tools.*` as a legacy alias.

## Summary

JP configures tool bindings under `conversation.tools.*`, even though those
bindings belong to the assistant that uses them.
This RFD moves tool configuration to `assistant.tools.*` and retains
`conversation.tools.*` as a legacy input alias for backward compatibility.
It defines the schema change, the compatibility mapping, updates to the
negative-delta claims map, and which documents to update versus preserve as
historical records.

## Motivation

`conversation.tools.<name>` configures which tools an assistant can call and how
they behave, including their source, run policy, access, display, and options.
Because a conversation has a single assistant, the conversation's tools and the
assistant's tools are currently the same set.
Although the configuration key sits under `conversation`, every consumer (query
setup, tool rendering, enable and disable flags, and the tool coordinator)
treats it as assistant configuration.

Placing assistant-owned settings under `conversation` is misleading and blocks
conversations with multiple assistants.
Moving the canonical key to `assistant.tools.*` aligns the configuration path
with its owner without changing behavior for single-assistant conversations.

Delaying this rename increases migration cost as more code and documentation
bind to the old key.
`conversation.tools` already appears in roughly twenty accepted RFDs, the
configuration documentation, and the negative-delta claims map (RFD 070), which
enumerates `conversation.tools.*` field paths directly.
Migrating the key now bounds the change and establishes a clear compatibility
window for the legacy alias.

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

[RFD D54]: D54-multi-participant-conversations.md
