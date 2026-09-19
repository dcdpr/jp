# Enforce resolved tool argument constraints before every dispatch

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-21
- **Label**: domain=llm
- **Label**: package=jp_llm
- **Label**: type=bug

Tool-call validation checks missing and unknown names, but does not enforce
parameter types, enums, array item schemas, or nested value constraints.
It runs only for local tools; MCP and built-in tools bypass it.

The recorded provider fixtures already contain invalid calls that pass through
the test harness, including Cerebras arrays with numeric items, Google values
outside the declared enum, and llama.cpp JSON-looking strings where an array was
intended.

`conversation.tools.<name>.parameters` documents enums as allowed-value
constraints and supports forcing a value.
Those constraints must be enforced by JP rather than treated only as model
guidance.

Open PR #998 validates resolved schema definitions but does not validate
argument instances at dispatch time.

Acceptance criteria:

- Validate required fields, unknown fields, JSON types, complete-value enums,
  `items`, and nested `properties` against the resolved tool schema.
- Apply the same validation before local, MCP, and built-in dispatch.
- Normalize strict-provider `null` placeholders back to omission or the
  configured default before validation.
  Optional nullable values must not leak into a tool whose source schema does
  not accept `null`.
- Return the existing invalid-arguments tool response without invoking the
  target.
- Add tests using invalid calls taken from the Cerebras, Google, and llama.cpp
  fixtures.
- Add tests proving MCP and built-in parameter overrides are enforced.

## Comments

-----

- **From**: jp
- **Date**: 2026-09-11T09:24:02Z

Implemented the strict-provider omission-decoding portion in the working tree:
request-local plans are produced by schema conversion, attached to transient
tool-call starts, and consumed by EventBuilder before constructing
ToolCallRequest.
OpenAI (streaming and non-streaming), OpenRouter, and llama.cpp are wired.
Source-nullable values and required properties are preserved; no generic null
repair or MCP default injection was added.

Reference and general composition decoding remains explicitly deferred to
T-0h28vbn.
Shared dispatch validation and default-policy work in this ticket remain open.

The new decoding tests were observed failing before the fix and pass after it.
Existing strict-schema, event-builder, and CLI turn-loop tests pass.
