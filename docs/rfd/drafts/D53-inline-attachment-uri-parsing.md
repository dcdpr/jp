<!--
  This template is a starting point, not a constraint. Delete sections that
  don't apply, add sections that do, or restructure entirely. The only
  requirement is the metadata header (Status, Authors, Date).

  Use HTML comments like this one for draft-time notes and review markers.
  They do not appear in the rendered output and can be removed when the RFD
  advances to Discussion status.
-->

# RFD D53: Inline Attachment URI Parsing

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-07
- **Required by**: [RFD D54]
- **Summary**: Parses registered URI schemes inline from `jp query` prompts into
  attachments and replaces `@path` shorthands with `file:` URIs.

## Summary

This RFD proposes parsing inline URIs (`file:`, `https:`, `jp:`, and other
registered schemes) in `jp query` prompts and routing matches to their
attachment handlers when `providers.resource.<scheme>.parse_query` is enabled.
It also replaces the `@path` shorthand in `--cfg @path` and query text with
explicit `file:` references.

## Motivation

Attaching context to a query requires a separate `jp attachment add` command or
a handler-specific flag, even though users routinely reference the target
resource (such as a file path, an HTTP URL, or a `jp://` conversation link)
directly in the prompt.
Parsing these references inline lets prompts carry their own context without a
second command.

JP already dispatches attachment handlers by URI scheme, so inline parsing can
route each scheme to its handler directly.
A per-scheme `conversation.attachments.<scheme>.auto_attach` setting controls
whether a matching URI in the prompt is attached automatically or left as plain
text.
This setting is independent of the handlers: registering a new scheme requires
no parser changes, and toggling `auto_attach` requires no handler changes.

Inline URI parsing also replaces the `@path` shorthand in query text and `--cfg
@path`, both of which overload the `@` prefix.
Standardizing file references on the `file:` prefix frees `@` and gives both
surfaces one consistent rule: `file:` points to a file, whether attached to a
prompt or loaded as configuration.

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
