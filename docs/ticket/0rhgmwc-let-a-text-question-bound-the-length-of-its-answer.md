# Let a text question bound the length of its answer

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-09-25
- **Label**: domain=llm
- **Label**: package=jp_cli
- **Label**: package=jp_mcp
- **Label**: package=jp_tool
- **Label**: type=follow-up

A text `Question` only says its answer is a string, so the schema an
assistant-targeted inquiry is asked with (`create_inquiry_schema` in `jp_cli`)
is `{"answer": {"type": "string"}}`.
Any string passes it.

`ticket_create` asks `shorter_title` of the assistant, and needs an answer of at
most 60 characters (`options.max_title_length`).
In a manual run (PR #1175), the inquiry model answered with a 213-character
report in the right shape, and the tool could only refuse it after the fact.

Let a text question carry a maximum length, e.g.
`Question::text(...).with_max_chars(60)`, and carry it through:

- `create_inquiry_schema` emits `maxLength` for the `answer` property.
- `InputRequest::schema()` includes it, so the JP MCP Server's answer check
  (`ask_for_input`, which already validates against that schema) refuses an
  overlong answer before the tool sees it.
- A user prompt can show the limit, or refuse to submit past it.

Open: whether Anthropic's (and other providers') structured output enforces
`maxLength` during generation or only describes it.
Check before relying on it for anything beyond the refusal.

Overlaps draft D13 (schema answer types).
