# RFD 028: Structured Inquiry System for Tool Questions

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-04
- **Extended by**: [RFD 034]
- **Summary**: Answers assistant-targeted tool questions via async
  structured-output inquiries instead of re-invoking the tool call.

## Summary

This RFD replaces the `tool_answers` re-invocation pattern with a structured
inquiry system.
When a tool requests input from the assistant, `ToolCoordinator` runs a
structured-output inquiry against the LLM, extracts the answer, and re-executes
the tool internally instead of having the LLM re-call the tool with its original
arguments and answers.

## Motivation

When a tool returns `NeedsInput` with `QuestionTarget::Assistant`, the system
must get an answer from the LLM without prompting the user.
The original approach re-invokes the tool across turns:

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
  re-transmitted in full.
  A tool with 5000-token arguments wastes ~5000 tokens per question.
- **Latency.** The LLM must re-generate the entire tool call, including all
  original arguments.
- **Error-prone.** The LLM can corrupt arguments when reconstructing them or
  misuse the internal `tool_answers` field.
  Either mistake contaminates conversation history and causes argument
  validation failures in subsequent tool calls.

Embedding `tool_answers` inside the `arguments` map of `ToolCallRequest` via
custom serde logic creates a hidden coupling.
The deserializer injects `tool_answers` into `arguments` even when empty,
providers pass `arguments` to the LLM in conversation history, and the LLM
learns to include `tool_answers` in new tool calls, which triggers "unknown
argument" validation errors.

## Design

### Overview

When a tool returns `NeedsInput` with `QuestionTarget::Assistant`,
`ToolCoordinator` spawns an async inquiry task instead of returning control to
the LLM.
The task makes a structured-output request for the answer, then re-executes the
tool with the accumulated answers.
The turn loop, the terminal, and the persisted conversation stream see none of
this: the tool simply takes longer to complete.

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

| Goal                   | Description                                  |
| ---------------------- | -------------------------------------------- |
| Token efficiency       | No re-transmission of tool arguments         |
| Type safety            | Structured-output schemas guarantee answer   |
|                        | types                                        |
| Full context           | LLM sees conversation history when answering |
| Cache-friendly         | Conversation prefix overlaps with main       |
|                        | stream                                       |
| Invisible to turn loop | Inquiry is encapsulated in `ToolCoordinator` |
| No rendering artifacts | Inquiry runs in a background task with       |
|                        | nothing printed                              |
| No stream mutation     | Real conversation stream stays untouched     |
|                        | during inquiry                               |
| Parallel inquiries     | Multiple tools can run concurrent inquiries  |

### InquiryBackend Trait

`InquiryBackend` abstracts inquiry execution for testability:

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

`LlmInquiryBackend` is constructed once per turn and holds an `Arc<dyn
Provider>`, the model details, and the static thread parts (system prompt,
sections, and attachments).

The `inquire` method:

1. Creates a JSON schema from the question type (boolean, select, text).
2. Appends a `ChatRequest` with the question text and schema to the cloned
   stream.
3. Makes a structured-output call to the LLM (with retry support).
4. Extracts and validates the answer from the structured response.

For tests, `MockInquiryBackend` returns pre-configured answers keyed by inquiry
ID.

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

The `inquiry_id` field uses `const`.
Providers that do not support `const` in JSON schemas transform it into a
single-value `enum`.

### ToolCoordinator Integration

Structured inquiries follow the same pattern as interactive user prompts in
`ToolCoordinator`:

```txt
User prompt (existing):
  NeedsInput → spawn_blocking: prompt user → PromptAnswer → re-execute

Structured inquiry (new):
  NeedsInput + QuestionTarget::Assistant → tokio::spawn: inquire()
    → InquiryResult → re-execute
```

`execute_with_prompting` takes an `Arc<dyn InquiryBackend>` and a
`&ConversationStream` reference.
When `handle_tool_result` encounters `NeedsInput` with
`QuestionTarget::Assistant`, it calls `spawn_inquiry` to snapshot the
conversation and run the inquiry task.

The `InquiryResult` event handler mirrors `PromptAnswer`: it inserts the answer
into `accumulated_answers` and re-spawns tool execution, or marks the tool as
completed with an error if the inquiry fails.

### Conversation Snapshot

An inquiry needs the current conversation events for context.
Rather than cloning the stream eagerly, `ToolCoordinator` holds a
`&ConversationStream` reference and clones it on demand when an inquiry fires,
avoiding upfront cost on cycles where no tool asks a question.

`spawn_inquiry` inserts a synthetic `ToolCallResponse` (`"Tool paused: ..."`)
into the cloned stream so the LLM sees a well-formed, resolved tool call pair,
then appends the question as a `ChatRequest` with the inquiry schema attached.

### Removal of `tool_answers`

The inquiry system makes the `tool_answers` field obsolete:

- `ToolCallRequest` drops the custom `Serialize` and `Deserialize`
  implementations that extracted and injected `tool_answers` in `arguments`, and
  derives serde directly.
- `storage.rs` drops the `Field::Map("tool_answers")` base64 encoding.
- Deserializing old conversation files silently ignores `tool_answers` through
  serde's default non-strict handling of unknown fields.

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

**Multi-question:** Each question triggers the same cycle, and answers
accumulate across re-executions.

**Parallel tools:** Each tool's inquiry runs as an independent async task.
The event channel serializes results as they arrive.

## Drawbacks

- **Extra LLM call per question.** Each inquiry makes a separate
  structured-output request, which adds up if a tool asks many questions.
  In practice, most tools ask 0 or 1 questions.
- **No batching.** Concurrent tool questions each fire a separate inquiry.
  Batching questions into a single request is left for future work.

## Alternatives

### Keep the `tool_answers` re-invocation pattern

Let the LLM re-call the tool with all arguments plus answers.

Rejected: wastes tokens proportional to argument size, adds latency, and
contaminates conversation history with the `tool_answers` field in `arguments`.

### Route answers through the main conversation stream

Instead of a side-channel inquiry, insert the question into the real
conversation stream and let the normal turn loop handle it.

Rejected: exposes internal questions to the user, mutates the persisted stream,
and breaks the `ToolCallRequest` → `ToolCallResponse` pairing that providers
expect.

## Non-Goals

- **Rendering inquiry events in `conversation show`.** Inquiries are invisible
  to the user; display formatting is deferred (see [RFD 005]).
- **Batching multiple inquiries** into a single structured request.
- **Non-tool inquiry sources.** The infrastructure supports conversation
  disambiguation and confirmation prompts, but this RFD only covers tool
  questions.

## Implementation Plan

### Phase 1: Inquiry infrastructure

Add `ActiveInquiry` (schema generation and answer extraction),
`create_inquiry_schema`, `tool_call_inquiry_id`, and unit tests.

Location: `crates/jp_cli/src/cmd/query/tool/inquiry.rs`

Can be merged independently.

### Phase 2: InquiryBackend trait and LLM implementation

Define the `InquiryBackend` trait, implement `LlmInquiryBackend` and
`MockInquiryBackend`, and add unit tests with a mock provider.

Can be merged independently.

### Phase 3: ToolCoordinator integration

Add the `InquiryResult` variant to `ExecutionEvent` and `spawn_inquiry` to
`ToolCoordinator`.
Wire `inquiry_backend` and `inquiry_events` into `execute_with_prompting`, and
handle `InquiryResult` in the event loop.

Depends on Phase 2.

### Phase 4: Turn loop wiring

Convert `Box<dyn Provider>` to `Arc<dyn Provider>`, construct
`LlmInquiryBackend` once before the main loop, and pass it to
`execute_with_prompting` at each `Executing` phase.

Depends on Phase 3.

### Phase 5: Cleanup

Remove the `tool_answers` serde machinery from `ToolCallRequest`, the
`tool_answers` base64 encoding from `storage.rs`, and
`pending_tool_call_questions` from `TurnState`.
Route non-interactive user-targeted questions through the inquiry backend.

Depends on Phase 4.

## References

- [RFD 005: First-Class Inquiry Events][RFD 005]: Recording
  `InquiryRequest`/`InquiryResponse` in the persisted stream.
- [RFD 009: Stateful Tool Protocol][RFD 009]: Future protocol for long-running
  tools (builds on inquiry infrastructure).
- [RFD 018: Typed Prompt Routing Enum][RFD 018]: Related follow-up work.

[RFD 005]: 005-first-class-inquiry-events.md
[RFD 009]: 009-stateful-tool-protocol.md
[RFD 018]: 018-typed-prompt-routing-enum.md
[RFD 034]: 034-inquiry-specific-assistant-configuration.md
