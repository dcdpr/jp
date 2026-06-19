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

## Summary

`jp query` treats its prompt as opaque text.
This RFD parses the prompt for URIs — `file:`, `https:`, `jp:`, and other
registered schemes — and routes each to the matching attachment handler, gated
per scheme by `providers.resource.<scheme>.parse_query = true | false`.
It also retires the overloaded `@path` forms (`--cfg @path` and the query-text
`@path` shorthand) in favor of `file:` references.

## Motivation

Attaching context to a query today means a separate `jp attachment add` step or
a handler-specific flag.
But users routinely name the thing they want attached directly in the prompt —
a file path, an HTTP URL, a `jp://` conversation link.
Parsing those references inline lets the prompt carry its own context instead of
requiring a second command.

A scheme-addressed model fits JP's attachment handlers, which are already
dispatched by URI scheme.
Each scheme maps to a handler, and a per-scheme
`conversation.attachments.<scheme>.auto_attach` toggle controls whether a bare
reference in the prompt is attached automatically or left as plain text.
The toggle stays orthogonal to the handlers: adding a scheme does not touch the
parser, and changing `auto_attach` does not touch any handler.

Inline parsing also forces an overdue cleanup.
The query-text `@path` shorthand and the `--cfg @path` config-path form both
overload `@`.
Standardizing file references on a `file:` prefix frees `@` and gives both
surfaces one consistent rule — `file:` points at a file, whether it is being
attached to the prompt or loaded as configuration.

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
