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

Implemented in the working tree as the record-and-answer design: the call is
recorded and answered with an error, and no executor is created for it.

- `EventBuilder::handle_flush` returns `Result<Option<ConversationEvent>,
  InvalidToolCall>`.
  Argument text that is truncated, not JSON, or JSON of another type is an
  `InvalidToolCall`; empty text and `{}` are a call without arguments.
- `TurnCoordinator` records the call as a `ToolCallRequest` without arguments,
  keeping the provider's metadata, and surfaces
  `CommittedEvent::InvalidToolCall`.
- `ToolCoordinator::reject` settles it on arrival with an error that names the
  parse failure and quotes the last 200 characters of the argument text.

Decisions made along the way:

- **Restart.** A restart submits every call without a response, and the request
  recorded for a rejected call reads as a call without arguments.
  The tool coordinator keeps rejected ids until the next response starts, and
  `prepare_one` refuses them.
- **`null`.** Rejected in the builder like any non-object.
  Google and Ollama hold parsed arguments and re-serialize them, and the Gemini
  client reads a call without `args` as `null`, so both providers emit `{}` for
  `null`.
- **Empty id or name.** A rejected call without an id or a name is dropped with
  a warning, since it cannot be answered.
  A call with well-formed arguments and an empty id or name is unchanged: an
  OpenAI-compatible server that omits ids works today for a single call, and
  rejecting those would break it.

Not covered here:

- A response cut off after a call's `Start` but before any argument text still
  flushes an empty buffer, which reads as a call without arguments.
  The builder cannot tell it from a real one; it needs the finish reason, which
  is T-0eyhkvs territory.
- A rejected call shows nothing on the terminal while the turn runs, as with an
  unavailable tool.
  It appears on replay.
- A model that truncates the same call on every attempt loops until T-0n0pmyh
  bounds the cycle count.
