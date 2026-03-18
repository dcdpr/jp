# RFD 028: Structured Inquiry System for Tool Questions

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-04
- **Extended by**: [RFD 034](034-inquiry-specific-assistant-configuration.md)

## Summary

This RFD replaces the `tool_answers` re-invocation pattern — where the LLM
re-calls a tool with all original arguments plus answers — with a structured
inquiry system. When a tool needs additional input targeted at the assistant,
the `ToolCoordinator` spawns an async task that makes a structured output
request to the LLM, extracts the answer, and re-executes the tool internally.
The LLM never re-transmits the tool's arguments.

## Motivation

When a tool returns `NeedsInput` with `QuestionTarget::Assistant`, the system
needs to get an answer from the LLM without the user's involvement. The original
approach works like this:

```txt
1. Assistant: ToolCallRequest(call_123, fs_modify_file, args={path, patterns})
2. Tool executes → NeedsInput("Create backup files?")
3. System: ToolCallResponse(call_123, "Tool needs input: ...")
4. Assistant: ToolCallRequest(call_456, fs_modify_file, args={path, patterns, tool_answers={...}})
   ^ Re-transmits ALL arguments
5. Tool completes
6. System: ToolCallResponse(call_456, "File modified successfully")
```

This has three problems:

- **Token waste.** Tool arguments (file contents, pattern lists) are
  re-transmitted in full. A tool with 5000-token arguments wastes ~5000 tokens
  per question.
- **Latency.** The LLM must re-generate the entire tool call, including all
  original arguments.
- **Error-prone.** The LLM might make mistakes reconstructing arguments, change
  values, or include the internal `tool_answers` field incorrectly — which
  contaminates conversation history and causes argument validation failures in
  subsequent tool calls.

The `tool_answers` field is embedded inside the `arguments` map of
`ToolCallRequest` and threaded through custom serde logic. This creates a hidden
coupling: the serialization layer injects `tool_answers` into `arguments` during
deserialization (even when empty), providers pass `arguments` to the LLM in
conversation history, and the LLM learns to include `tool_answers` in new tool
calls — triggering "unknown argument" validation errors.

## Design

### Overview

When a tool returns `NeedsInput` with `QuestionTarget::Assistant`, the
`ToolCoordinator` spawns an async inquiry task instead of returning control to
the LLM. The task makes a structured output request to get the answer, then
re-executes the tool with accumulated answers. The turn loop, the terminal, and
the persisted conversation stream see none of this — the tool simply takes
longer to complete.

```txt
1. Assistant: ToolCallRequest(call_123, fs_modify_file, args={path, patterns})
2. Tool executes → NeedsInput("Create backup files?")
   — Inquiry runs as async task (invisible to turn loop) —
3. LLM returns structured response: {"inquiry_id": "...", "answer": true}
4. Tool re-executes with answer → completes
   — Turn loop sees only the final result —
5. System: ToolCallResponse(call_123, "File modified successfully")
```

### Design Goals

| Goal                   | Description                              |
|------------------------|------------------------------------------|
| Token efficiency       | No re-transmission of tool arguments     |
| Type safety            | Structured output schemas guarantee      |
|                        | answer types                             |
| Full context           | LLM sees conversation history when       |
|                        | answering                                |
| Cache-friendly         | Conversation prefix overlaps with main   |
|                        | stream                                   |
| Invisible to turn loop | Inquiry is encapsulated in               |
|                        | `ToolCoordinator`                        |
| No rendering artifacts | Inquiry runs in a background task,       |
|                        | nothing printed                          |
| No stream mutation     | Real conversation stream untouched       |
|                        | during inquiry                           |
| Parallel inquiries     | Multiple tools can have concurrent       |
|                        | inquiries                                |

### InquiryBackend Trait

The inquiry backend is a trait for testability:

```rust
#[async_trait]
pub trait InquiryBackend: Send + Sync {
    async fn inquire(
        &self,
        events: ConversationStream,
        inquiry_id: &str,
        question: &Question,
        cancellation_token: CancellationToken,
    ) -> Result<Value, InquiryError>;
}
```

The real implementation (`LlmInquiryBackend`) holds an `Arc<dyn Provider>`, the
model details, and the static thread parts (system prompt, sections,
attachments). These are set once per turn.

The `inquire` method:

1. Creates a JSON schema from the question type (boolean, select, text).
2. Appends a `ChatRequest` with the question text and schema to the cloned
   stream.
3. Makes a structured output call to the LLM (with retry support).
4. Extracts and validates the answer from the structured response.

A `MockInquiryBackend` returns pre-configured answers keyed by inquiry ID for
testing purposes.

### Schema Generation

Each inquiry generates a schema matching the question type:

```json
{
  "type": "object",
  "properties": {
    "inquiry_id": {
      "type": "string",
      "const": "tool_call.fs_modify_file.call_a3b7"
    },
    "answer": {
      "type": "boolean"
    }
  },
  "required": [
    "inquiry_id",
    "answer"
  ],
  "additionalProperties": false
}
```

The `inquiry_id` field uses `const`, but individual providers will transform the
schema to use s single-value `enum` if the provider does not support `const` in
the schema.

### ToolCoordinator Integration

The inquiry follows the same pattern as interactive user prompts, which already
exist in the coordinator:

```txt
User prompt (existing):
  NeedsInput → spawn_blocking: prompt user → PromptAnswer → re-execute

Structured inquiry (new):
  NeedsInput + QuestionTarget::Assistant → tokio::spawn: inquire()
    → InquiryResult → re-execute
```

`execute_with_prompting` takes an `Arc<dyn InquiryBackend>` and a
`&ConversationStream` reference. The stream is only cloned if an inquiry
actually fires — no upfront cost on cycles where no tool asks a question.

When `handle_tool_result` encounters `NeedsInput` with
`QuestionTarget::Assistant`, it calls `spawn_inquiry`, which inserts a synthetic
`ToolCallResponse` ("Tool paused: ...") into the cloned stream so the LLM sees
the tool call as resolved, then appends the question as a `ChatRequest` with a
schema attached.

The `InquiryResult` event handler mirrors `PromptAnswer`: insert the answer into
`accumulated_answers`, re-spawn tool execution. On failure, mark the tool as
completed with an error.

### Conversation Snapshot

The inquiry needs the current conversation events for context. Rather than
cloning the stream eagerly, the coordinator holds a `&ConversationStream`
reference and clones just-in-time when an inquiry fires. The clone gets a
synthetic `ToolCallResponse` inserted so the LLM sees a well-formed tool call
pair.

### Removal of `tool_answers`

With the inquiry system in place, the `tool_answers` field is no longer needed:

- The custom `Serialize`/`Deserialize` on `ToolCallRequest` that
  extracts/injects `tool_answers` from/into `arguments` is removed.
  `ToolCallRequest` uses derive serde instead.
- The `Field::Map("tool_answers")` base64 encoding in `storage.rs` is removed.
- Deserialization of old conversation files that contain a `tool_answers` field
  silently ignores it (serde default behavior for unknown fields with non-strict
  deserialization).

### Data Flow

**Single question:**

```txt
ToolCoordinator                          Inquiry task (tokio::spawn)
─────────────                            ────────────
Tool executes
  → NeedsInput(Q1, Assistant)
  spawn inquiry                   →      inquire(stream, Q1)
                                           structured output call
                                  ←      InquiryResult(answer)
merge answer, re-execute tool
  → Completed(response)
```

**Multi-question:** Each question triggers the same cycle. Answers accumulate
across re-executions.

**Parallel tools:** Each tool's inquiry runs as an independent async task. The
event channel serializes results as they arrive.

## Drawbacks

- **Extra LLM call per question.** Each inquiry is a separate structured output
  request. For tools that ask many questions, this adds up. In practice, most
  tools ask 0-1 questions.
- **No batching.** If multiple tools need answers simultaneously, each fires its
  own inquiry. Batching questions into a single request is left as a future
  enhancement.

## Alternatives

### Keep the `tool_answers` re-invocation pattern

Let the LLM re-call the tool with all arguments plus answers.

Rejected: wastes tokens proportional to argument size, introduces latency, and
the `tool_answers` field in `arguments` causes history contamination.

### Route answers through the main conversation stream

Instead of a side-channel inquiry, insert the question into the real
conversation stream and let the normal turn loop handle it.

Rejected: this would be visible to the user, would modify the persisted stream,
and would break the `ToolCallRequest` → `ToolCallResponse` pairing that
providers expect.

## Non-Goals

- **Rendering inquiry events in `conversation show`.** The inquiry is invisible.
  Display formatting is deferred (see [RFD 005]).
- **Batching multiple inquiries** into a single structured request.
- **Non-tool inquiry sources** (conversation disambiguation, confirmation
  prompts). The infrastructure supports this but it is not proposed here.

## Implementation Plan

### Phase 1: Inquiry infrastructure

Add `ActiveInquiry` (schema generation + answer extraction),
`create_inquiry_schema`, `tool_call_inquiry_id`, and unit tests.

Location: `crates/jp_cli/src/cmd/query/tool/inquiry.rs`

Can be merged independently.

### Phase 2: InquiryBackend trait and LLM implementation

Define the `InquiryBackend` trait. Implement `LlmInquiryBackend` and
`MockInquiryBackend`. Unit tests with mock provider.

Can be merged independently.

### Phase 3: ToolCoordinator integration

Add `InquiryResult` variant to `ExecutionEvent`. Add `spawn_inquiry` to
`ToolCoordinator`. Wire `inquiry_backend` and `inquiry_events` into
`execute_with_prompting`. Handle `InquiryResult` in the event loop.

Depends on Phase 2.

### Phase 4: Turn loop wiring

Convert `Box<dyn Provider>` to `Arc<dyn Provider>`. Construct
`LlmInquiryBackend` once before the main loop. Pass it to
`execute_with_prompting` at each `Executing` phase.

Depends on Phase 3.

### Phase 5: Cleanup

Remove `tool_answers` serde machinery from `ToolCallRequest`. Remove
`tool_answers` base64 encoding from `storage.rs`. Remove
`pending_tool_call_questions` from `TurnState`. Route non-interactive
user-targeted questions through the inquiry backend.

Depends on Phase 4.

## References

- [RFD 005: First-Class Inquiry Events][RFD 005] — recording
  `InquiryRequest`/`InquiryResponse` in the persisted stream.
- [RFD 009: Stateful Tool Protocol][RFD 009] — future protocol for long-running
  tools (builds on inquiry infrastructure).
- [RFD 018: Typed Prompt Routing Enum][RFD 018] — related follow-up work.

[RFD 005]: 005-first-class-inquiry-events.md
[RFD 009]: 009-stateful-tool-protocol.md
[RFD 018]: 018-typed-prompt-routing-enum.md
