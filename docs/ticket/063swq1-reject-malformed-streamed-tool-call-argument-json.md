# Reject malformed streamed tool-call argument JSON

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-08-21
- **Label**: domain=llm
- **Label**: package=jp_cli
- **Label**: package=jp_llm
- **Label**: type=bug

`EventBuilder` converts malformed tool-call argument JSON into an empty map and
emits a normal `ToolCallRequest` (`crates/jp_llm/src/event_builder.rs`).
A truncated provider stream can therefore execute a no-argument tool or a tool
whose defaults fill the missing fields.

Acceptance criteria:

- A tool-call buffer containing malformed JSON must not become a valid empty
  argument map.
- Surface a specific invalid-call response or stream error that can be recorded
  and returned to the model.
- Prove that the tool executor is not invoked.
- Cover truncated JSON, a non-object top-level value, and valid `{}` separately.
- Add a test that fails against the current fallback behavior.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-13T14:54:05Z

Implemented the record-and-answer design: an invalid tool call is recorded with
empty arguments and answered with an error response, without creating an
executor.

- `EventBuilder::handle_flush` returns `Result<Option<ConversationEvent>,
  InvalidToolCall>`.
  Argument text that is truncated, malformed, or not a JSON object returns
  `Err(InvalidToolCall)`; empty text and `{}` produce a `ToolCallRequest` with
  an empty argument map.
- `TurnCoordinator` records the call as a `ToolCallRequest` with empty arguments
  and the provider's metadata, then surfaces `CommittedEvent::InvalidToolCall`.
- `ToolCoordinator::reject` settles the call on arrival with an error response
  describing the parse failure and quoting the trailing 200 characters of the
  argument text.

Design notes:

- **Restart.** Restarting tool calls re-submits every unresponded call, and the
  request recorded for a rejected call has empty arguments.
  `ToolCoordinator` retains rejected call IDs until the next response starts so
  `prepare_one` returns the stored rejection instead of executing the tool.
- **`null`.** `EventBuilder` rejects literal `"null"` like any other non-object
  value.
  Because Google and Ollama deserialize parameterless tool calls with `null`
  arguments, both providers normalize `Value::Null` to `"{}"` when emitting
  `ArgumentChunk`.
- **Empty ID or name.** An `InvalidToolCall` with an empty ID or tool name is
  dropped with a warning because no response can be paired with it.
  Valid tool calls with an empty ID or name remain unchanged so single-call
  responses from OpenAI-compatible servers that omit IDs continue to work.

Out of scope:

- A stream truncated after `ToolCallPart::Start` but before any `ArgumentChunk`
  still flushes an empty buffer and is treated as a zero-argument call.
  Distinguishing that case requires inspecting `FinishReason` (tracked in
  T-0eyhkvs).
- A rejected call renders no terminal output during the live turn (matching
  unavailable tools), though it appears on replay.
- Bounding repeated truncation loops is tracked in T-0n0pmyh.
