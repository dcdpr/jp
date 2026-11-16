# swift_test_ui is skipped: its tests parameter has no items

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-16
- **Label**: domain=tooling
- **Label**: type=bug

`swift_test_ui` is never offered to the assistant.
Its `tests` parameter in `.jp/mcp/tools/swift/test_ui.toml` is declared `type =
"array"` with no item schema, and schema resolution rejects that
(`crates/jp_tool/src/schema.rs`).
JP then leaves the tool out of the turn:

```text
WARN jp_mcp::server: Skipping tool: its parameter schema could not be resolved.
  tool: swift_test_ui
  error: Invalid schema at `conversation.tools.swift_test_ui.parameters.tests.items`: array schemas must declare an item schema
```

Seen in the trace log of `jp query --new -c dev ...` on 2026-11-16.
The dev persona enables the tool through
`.jp/config/skill/swift-development.toml`, so every dev-persona query hits it.
The run's terminal output showed nothing about it.

The parameter has had no `items` since the file was added (77f0c9f76).
The check that rejects it is from 5c50ed994, dated the same day.

## Fix

Add `items.type = "string"` to
`[conversation.tools.swift_test_ui.parameters.tests]`, as `swift_test`'s
`testname` and `fs_list_files`'s `prefixes` do.

## Related

A tool that the config enables and JP then drops is reported only in the trace
log.
Whether that deserves a visible notice is a separate question from this fix.
