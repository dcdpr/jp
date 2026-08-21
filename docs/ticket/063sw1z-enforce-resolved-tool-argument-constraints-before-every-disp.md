# Enforce resolved tool argument constraints before every dispatch

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-21

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

Open PR \#998 validates resolved schema definitions but does not validate
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
