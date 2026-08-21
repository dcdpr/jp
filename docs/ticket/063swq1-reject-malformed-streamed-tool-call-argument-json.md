# Reject malformed streamed tool-call argument JSON

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-21

`EventBuilder` converts malformed tool-call argument JSON into an empty map and emits a normal `ToolCallRequest` (`crates/jp_llm/src/event_builder.rs`). A truncated provider stream can therefore execute a no-argument tool or a tool whose defaults fill the missing fields.

Acceptance criteria:

- A tool-call buffer containing malformed JSON must not become a valid empty argument map.
- Surface a specific invalid-call response or stream error that can be recorded and returned to the model.
- Prove that the tool executor is not invoked.
- Cover truncated JSON, a non-object top-level value, and valid `{}` separately.
- Add a test that fails against the current fallback behavior.
