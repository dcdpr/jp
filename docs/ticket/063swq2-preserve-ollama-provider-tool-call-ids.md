# Preserve Ollama provider tool-call IDs

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-21

Recorded Ollama responses contain provider-generated call IDs such as
`call_77bh2121`, but JP discards them and persists synthetic IDs such as
`run_me_2`.
The pinned `ollama-rs` response type exposes only the function payload, so the
ID is lost during deserialization.

Acceptance criteria:

- Preserve the provider ID when Ollama sends one.
- Use a deterministic synthetic ID only for responses from older servers that
  omit the field.
- Keep parallel calls and repeated calls across turns distinct.
- Round-trip the same ID through `ToolCallRequest`, `ToolCallResponse`, and the
  next Ollama request.
- Update the Ollama fixtures to assert the recorded provider IDs.
- If this requires an `ollama-rs` change, pin the fixed revision and add an
  upstream regression test.
