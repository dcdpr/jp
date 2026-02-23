# Stateful Tool Inquiries Architecture

This document describes the architecture for handling tool questions via
structured output inquiries, eliminating the need to re-transmit tool
arguments when a tool requires additional input.

## Table of Contents

- [Overview](#overview)
- [Motivation](#motivation)
- [Design Goals](#design-goals)
- [Architecture Overview](#architecture-overview)
  - [Responsibilities](#responsibilities)
  - [Integration with Existing Patterns](#integration-with-existing-patterns)
- [Implementation Details](#implementation-details)
  - [Conversation Snapshot](#conversation-snapshot)
  - [ToolCoordinator Changes](#toolcoordinator-changes)
  - [ExecutionEvent Changes](#executionevent-changes)
  - [The inquire Function](#the-inquire-function)
  - [Turn Loop Changes](#turn-loop-changes)
  - [Provider Ownership](#provider-ownership)
  - [Schema Generation](#schema-generation)
  - [Schema Compatibility](#schema-compatibility)
  - [Inquiry ID Format](#inquiry-id-format)
  - [Answer Extraction](#answer-extraction)
- [Data Flow](#data-flow)
  - [Single Question Flow](#single-question-flow)
  - [Multi-Question Flow](#multi-question-flow)
  - [Parallel Tool Calls with Inquiries](#parallel-tool-calls-with-inquiries)
- [Stream and Rendering](#stream-and-rendering)
- [Signal Interrupts](#signal-interrupts)
- [Error Handling](#error-handling)
- [Provider Caching](#provider-caching)
- [Token Efficiency](#token-efficiency)
- [Testing Strategy](#testing-strategy)
- [Migration Path](#migration-path)

---

## Overview

When a tool returns `Outcome::NeedsInput` with
`QuestionTarget::Assistant`, the `ToolCoordinator` spawns an async
task that makes a structured output request to the LLM, gets the
answer, and sends it back via the existing event channel. The tool is
then re-executed with the answer. This cycle repeats for multi-question
tools until the tool completes.

The inquiry is invisible to the turn loop, the terminal, and the
persisted conversation stream. From the turn loop's perspective, the
tool simply takes longer to complete.

---

## Motivation

### Current Flow (Inefficient)

```
1. User: "Modify file X"
2. Assistant: ToolCallRequest(call_123, fs_modify_file, args={path, patterns})
3. Tool executes -> NeedsInput("Create backup files?")
4. System: ToolCallResponse(call_123, "Tool needs input: ...")
5. Assistant: ToolCallRequest(call_456, fs_modify_file, args={path, patterns, tool_answers})
   ^ Re-transmits ALL arguments (potentially thousands of tokens)
6. Tool completes
7. System: ToolCallResponse(call_456, "File modified successfully")
```

Problems:

- **Token waste**: Tool arguments (file contents, pattern lists) are
  re-transmitted in full.
- **Latency**: The LLM must re-generate the entire tool call.
- **Error-prone**: The LLM might make mistakes reconstructing
  arguments.

### New Flow (Efficient)

```
1. User: "Modify file X"
2. Assistant: ToolCallRequest(call_123, fs_modify_file, args={...})
3. Tool executes -> NeedsInput("Create backup files?")
   --- Inquiry runs as async task (invisible to turn loop) ---
4. ToolCoordinator spawns inquiry task
5. LLM returns: {"inquiry_id": "...", "answer": true}
6. Tool re-executes with answer -> completes
   --- Turn loop sees only the final result ---
7. System: ToolCallResponse(call_123, "File modified successfully")
```

---

## Design Goals

| Goal                       | Description                                    |
|----------------------------|------------------------------------------------|
| **Token efficiency**       | Avoid re-transmitting tool arguments            |
| **Type safety**            | Structured output schemas guarantee answer      |
|                            | types                                           |
| **Full context**           | LLM sees conversation history when answering    |
| **Cache-friendly**         | Conversation prefix matches, enabling provider  |
|                            | caching                                         |
| **No turn loop changes**   | Inquiry is encapsulated in ToolCoordinator;     |
|                            | turn loop is unaware                            |
| **No rendering artifacts** | Inquiry runs in a separate task; nothing is     |
|                            | printed to the terminal                         |
| **No stream mutation**     | Real conversation stream is not modified during |
|                            | inquiry; snapshot is used for context            |
| **Parallel inquiries**     | Multiple tools can have concurrent inquiries    |

---

## Architecture Overview

### Responsibilities

**ToolCoordinator**: Executes tools. When a tool returns `NeedsInput`
with `QuestionTarget::Assistant`, it spawns an async inquiry task that
calls the LLM via structured output. When the answer arrives via the
event channel, it re-executes the tool with accumulated answers. This
mirrors how user prompts already work in the coordinator.

**Turn Loop** (`run_turn_loop`): Constructs the provider, model, and
conversation snapshot, and passes them to the `ToolCoordinator`. Calls
`execute_with_prompting` and gets back finished results. Has no
knowledge of inquiries.

### Integration with Existing Patterns

The inquiry follows the same pattern as interactive user prompts,
which already exist in the `ToolCoordinator`:

```
User Prompt (existing):
  Tool returns NeedsInput
  -> spawn_blocking: prompt user on terminal
  -> send PromptAnswer via channel
  -> re-execute tool with answer

Structured Inquiry (new):
  Tool returns NeedsInput + QuestionTarget::Assistant
  -> tokio::spawn: inquiry::inquire()
  -> send InquiryAnswer via channel
  -> re-execute tool with answer
```

The event channel, the re-execution logic, and the answer merging are
the same. Only the source of the answer differs: terminal prompt vs
LLM structured output call.

The `InquiryAnswer` event handler follows a similar merge-and-
reexecute pattern as `PromptAnswer`, but is handled inline rather
than extracted into a shared helper. Inquiries do not interact
with the prompt queue or persist levels, so the shared core is
just the answer insertion and tool re-spawn.

---

## Implementation Details

### Conversation Snapshot

The inquiry needs two things:

1. A `Thread` with the static parts (system prompt, sections,
   attachments). Built once per turn, owned by the
   `LlmInquiryBackend`.
2. The current `ConversationStream` (events). Cloned just-in-time
   only when an inquiry actually fires.

The `ToolCoordinator` receives a `&ConversationStream` reference
for the current execution phase. If a tool needs an inquiry, the
coordinator clones the stream at that moment and passes the owned
copy to the inquiry backend. This avoids cloning the entire
conversation on every execution cycle.

### ToolCoordinator Changes

The `ToolCoordinator` does not own inquiry state. Instead, the
inquiry backend and conversation events are passed as parameters
to `execute_with_prompting`:

```rust
pub async fn execute_with_prompting(
    &mut self,
    // ... existing params ...
    prompt_backend: &dyn PromptBackend,
    inquiry_backend: Arc<dyn InquiryBackend>,
    inquiry_events: &ConversationStream,
    // ...
) -> ExecutionResult
```

`inquiry_backend` is `Arc` (not `&dyn`) because inquiry tasks are
spawned with `tokio::spawn` and need `'static` data. This diverges
slightly from `prompt_backend` which is `&dyn` because prompts run
via `spawn_blocking` through the `Arc<ToolPrompter>` wrapper.

`inquiry_events` is a reference to the current conversation stream.
It is only cloned when an inquiry actually fires (just-in-time),
avoiding unnecessary copies on cycles where no tool asks a question.

When `handle_tool_result` encounters `NeedsInput` with
`QuestionTarget::Assistant`, it spawns the inquiry task directly:

```rust
// In handle_tool_result, NeedsInput branch:
if target == QuestionTarget::Assistant {
    let inquiry_id = inquiry::tool_call_inquiry_id(&tool_name, &tool_id);
    Self::spawn_inquiry(
        index,
        inquiry_id,
        question,
        Arc::clone(&inquiry_backend),
        inquiry_events.clone(),  // just-in-time clone
        cancellation_token.child_token(),
        event_tx.clone(),
    );
    self.set_tool_state(&tool_id, ToolCallState::AwaitingInput);
    return;
}
```

`handle_tool_result` remains synchronous. `spawn_inquiry` uses
`tokio::spawn` to kick off the async inquiry task, which does not
require the caller to be async — the same pattern used by the
existing `spawn_user_prompt` and `spawn_tool_execution` methods.
The `inquiry_backend` and `inquiry_events` are passed as
parameters to `handle_tool_result` (alongside the existing
parameters like `mcp_client`, `root`, etc.).

The `spawn_inquiry` function mirrors the existing
`spawn_user_prompt`:

```rust
fn spawn_inquiry(
    index: usize,
    inquiry_id: String,
    tool_call_id: String,
    question: Question,
    backend: Arc<dyn InquiryBackend>,
    mut events: ConversationStream,
    cancellation_token: CancellationToken,
    event_tx: mpsc::Sender<ExecutionEvent>,
) {
    // Insert a ToolCallResponse into the cloned stream so the LLM
    // sees the tool as "paused". The ID must match the original
    // ToolCallRequest.id so providers can resolve the tool name.
    events.push(ToolCallResponse {
        id: tool_call_id,
        result: Ok(format!("Tool paused: {}", question.text)),
    });

    let question_id = question.id.clone();
    tokio::spawn(async move {
        let result = backend
            .inquire(events, &inquiry_id, &question, cancellation_token)
            .await;

        match result {
            Ok(answer) => {
                let _ = event_tx
                    .send(ExecutionEvent::InquiryAnswer {
                        index,
                        question_id,
                        answer,
                    })
                    .await;
            }
            Err(err) => {
                let _ = event_tx
                    .send(ExecutionEvent::InquiryFailed {
                        index,
                        error: err.to_string(),
                    })
                    .await;
            }
        }
    });
}
```

### ExecutionEvent Changes

Two new variants on the existing `ExecutionEvent` enum:

```rust
enum ExecutionEvent {
    // ... existing variants ...

    /// Answer received from a structured inquiry (LLM).
    InquiryAnswer {
        index: usize,
        question_id: String,
        answer: Value,
    },

    /// Structured inquiry failed.
    InquiryFailed {
        index: usize,
        error: String,
    },
}
```

`InquiryAnswer` is handled inline in the event loop. The core
logic (insert answer into `accumulated_answers`, re-spawn tool
execution) mirrors `PromptAnswer` but is kept separate because
inquiries do not interact with the prompt queue (`prompt_active`),
persist levels, or `process_next_prompt`:

```rust
ExecutionEvent::InquiryAnswer { index, question_id, answer } => {
    if let Some(tool) = executing_tools.get_mut(&index) {
        tool.accumulated_answers.insert(question_id, answer);
        self.set_tool_state(&tool.tool_id, ToolCallState::Running);
        Self::spawn_tool_execution(
            index, tool.executor.clone(),
            tool.accumulated_answers.clone(),
            /* ... */
        );
    }
}

ExecutionEvent::InquiryFailed { index, error } => {
    if let Some(tool) = executing_tools.get(&index) {
        self.set_tool_state(&tool.tool_id, ToolCallState::Completed);
        results[index] = Some(ToolCallResponse {
            id: tool.tool_id.clone(),
            result: Err(format!("Inquiry failed: {error}")),
        });
    }
}
```

### InquiryBackend Trait

The inquiry backend is a trait for testability and separation of
concerns:

```rust
#[async_trait]
pub trait InquiryBackend: Send + Sync {
    /// Make a structured inquiry and return the answer.
    ///
    /// `events` is an owned conversation stream (cloned just-in-time
    /// by the caller). The implementation appends temporary events,
    /// builds a thread, makes the LLM call, and extracts the answer.
    ///
    /// `inquiry_id` is an opaque correlation ID for logging.
    /// `question` describes the expected answer type and text.
    async fn inquire(
        &self,
        events: ConversationStream,
        inquiry_id: &str,
        question: &Question,
        cancellation_token: CancellationToken,
    ) -> Result<Value, Error>;
}
```

The real implementation holds the provider, model, and the static
thread parts (set once per turn):

```rust
pub struct LlmInquiryBackend {
    provider: Arc<dyn Provider>,
    model: ModelDetails,
    system_prompt: Option<String>,
    sections: Vec<SectionConfig>,
    attachments: Vec<Attachment>,
}

#[async_trait]
impl InquiryBackend for LlmInquiryBackend {
    async fn inquire(
        &self,
        mut events: ConversationStream,
        inquiry_id: &str,
        question: &Question,
        cancellation_token: CancellationToken,
    ) -> Result<Value, Error> {
        let inquiry = ActiveInquiry::new(
            inquiry_id.to_string(),
            question.clone(),
        );

        // Append the user-facing question with the structured output
        // schema. The caller is responsible for any context events
        // (e.g. a ToolCallResponse) that precede this in the stream.
        events.push(ChatRequest {
            content: format!(
                "A tool requires additional input.\n\n{}\n\n\
                 Provide your answer based on the conversation \
                 context.",
                question.text,
            ),
            schema: Some(inquiry.schema.clone()),
        });

        // Build thread from static parts + provided events.
        let thread = Thread {
            system_prompt: self.system_prompt.clone(),
            sections: self.sections.clone(),
            attachments: self.attachments.clone(),
            events,
        };

        let query = ChatQuery {
            thread,
            tools: vec![],
            tool_choice: ToolChoice::None,
        };

        let retry_config = resilient_stream::RetryConfig::default();
        let llm_events = tokio::select! {
            biased;
            () = cancellation_token.cancelled() => {
                return Err(/* cancellation error */);
            }
            result = resilient_stream::collect_with_retry(
                self.provider.as_ref(),
                &self.model,
                query,
                &retry_config,
            ) => {
                result?
            }
        };

        let structured_data = llm_events
            .into_iter()
            .filter_map(Event::into_conversation_event)
            .filter_map(ConversationEvent::into_chat_response)
            .find_map(ChatResponse::into_structured_data)
            .ok_or(/* missing structured data error */)?;

        inquiry
            .extract_answer(&structured_data)
            .ok_or(/* extraction error */)
    }
}
```

For tests, a mock that returns pre-configured answers:

```rust
pub struct MockInquiryBackend {
    answers: HashMap<String, Value>,
}

#[async_trait]
impl InquiryBackend for MockInquiryBackend {
    async fn inquire(
        &self,
        _events: ConversationStream,
        inquiry_id: &str,
        _question: &Question,
        _cancellation_token: CancellationToken,
    ) -> Result<Value, Error> {
        self.answers
            .get(inquiry_id)
            .cloned()
            .ok_or(/* not found error */)
    }
}
```

### Turn Loop Changes

Minimal. The turn loop needs to:

1. Create the provider as `Arc<dyn Provider>` (see
   [Provider Ownership](#provider-ownership)).
2. Build `LlmInquiryBackend` once before the main loop.
3. Pass `Arc::clone(&inquiry_backend)` and a
   `&ConversationStream` reference to `execute_with_prompting`
   at each `Executing` phase. No snapshot or context setup is
   needed — the stream is only cloned if an inquiry fires.

```rust
// In run_turn_loop, before the main loop:
let provider: Arc<dyn Provider> = provider::get_provider(...)?;
let model = provider.model_details(&model_id.name).await?;

let inquiry_backend: Arc<dyn InquiryBackend> = Arc::new(
    LlmInquiryBackend::new(
        Arc::clone(&provider),
        model.clone(),
        cfg.assistant.system_prompt.clone(),
        /* sections built from cfg.assistant */,
        attachments.to_vec(),
    ),
);

// ... main loop ...

TurnPhase::Executing => {
    // Pass a reference to the current conversation events.
    // Only cloned if an inquiry actually fires.
    let inquiry_events = workspace
        .get_events(&conversation_id)
        .expect("conversation must exist");

    let execution_result = tool_coordinator
        .execute_with_prompting(
            // ... existing params ...
            Arc::clone(&inquiry_backend),
            inquiry_events,
        )
        .await;

    // ... rest unchanged ...
}
```

The turn loop does NOT need to handle inquiries, phases, stream
manipulation, or structured output responses.

### Provider Ownership

`get_provider` returns `Box<dyn Provider>`. The turn loop converts
it to `Arc<dyn Provider>` so the provider can be shared with
spawned inquiry tasks:

```rust
let provider: Arc<dyn Provider> = Arc::from(
    provider::get_provider(model_id.provider, &cfg.providers.llm)?,
);
```

`Arc::from(Box<T>)` is a zero-copy conversion. Call sites that use
`provider.as_ref()` or `&*provider` work the same way with `Arc`
as with `Box`. No changes to `get_provider` itself are needed.

### Schema Generation

Already implemented in Phase 1
(`crates/jp_cli/src/cmd/query/tool/inquiry.rs`).

Each inquiry generates a JSON schema matching the question type:

```json
{
  "type": "object",
  "properties": {
    "inquiry_id": {
      "type": "string",
      "const": "tool_call.fs_modify_file.call_a3b7c9d1"
    },
    "answer": {
      "type": "boolean"
    }
  },
  "required": ["inquiry_id", "answer"],
  "additionalProperties": false
}
```

### Schema Compatibility

Not all providers support `const` in JSON schemas. The schema is
generated with `const` as the ideal constraint, and providers rewrite
it as needed during their existing schema transformation step.

| Provider   | `const`    | `enum`  | Workaround          |
|------------|------------|---------|---------------------|
| Anthropic  | Yes        | Yes     | N/A                 |
| OpenAI     | Partial    | Yes     | Rewrite to `enum`   |
| Google     | No         | Yes     | Rewrite to `enum`   |
| Ollama     | No         | Maybe   | Description forcing  |

Rewrite strategy (applied per-provider in their schema transform):

1. **Ideal**: Provider supports `const` -- use as-is
2. **Fallback 1**: Provider supports `enum` -- rewrite
   `{"const": "x"}` to `{"enum": ["x"]}`
3. **Fallback 2**: Neither supported -- move the value into
   `description` as a strong hint

### Inquiry ID Format

Already implemented in Phase 1.

```
tool_call.<tool_name>.<tool_call_id>
```

### Answer Extraction

Already implemented in Phase 1. See `ActiveInquiry::extract_answer`.

---

## Data Flow

### Single Question Flow

```
ToolCoordinator event loop                Inquiry task (tokio::spawn)
------------------------------            --------------------------------
Tool executes
  -> NeedsInput(Q1, Assistant)
  spawn inquiry task               --->   inquiry::inquire(thread, Q1)
                                            build thread + temp events
                                            provider.chat_completion_stream()
                                            extract structured answer
                                   <---   send InquiryAnswer(answer_1)

receive InquiryAnswer
  merge answer_1 into accumulated
  re-execute tool with {Q1: a1}
  -> Completed(response)

collect response
```

### Multi-Question Flow

Each question triggers the same cycle:

```
Tool run 1: execute(answers={})          -> NeedsInput(Q1)
  Inquiry task: get answer_1
  InquiryAnswer received

Tool run 2: execute(answers={Q1: a1})    -> NeedsInput(Q2)
  Inquiry task: get answer_2
  InquiryAnswer received

Tool run 3: execute(answers={Q1: a1, Q2: a2}) -> Completed
  Collect final response
```

### Parallel Tool Calls with Inquiries

When multiple tools run in parallel and more than one needs an
inquiry, each fires its own inquiry task concurrently:

```
Tool A executes -> NeedsInput  -> spawn inquiry task A
Tool B executes -> Completed   -> result collected
Tool C executes -> NeedsInput  -> spawn inquiry task C

Inquiry task A returns answer  -> re-execute Tool A -> Completed
Inquiry task C returns answer  -> re-execute Tool C -> Completed

All tools done -> return ExecutionResult to turn loop
```

The event channel naturally serializes the answers as they arrive.
Each tool's re-execution is independent.

---

## Stream and Rendering

The inquiry is invisible to both the conversation stream and the
terminal:

- **Stream**: The real `ConversationStream` is not modified during the
  inquiry. The `inquire` function works on a cloned `Thread` snapshot.
  Only the final `ToolCallResponse` is added to the stream after
  `execute_with_prompting` returns and the turn loop processes the
  results.

- **Rendering**: The inquiry runs in a spawned async task. It does not
  go through `ChatResponseRenderer`, `StructuredRenderer`, or any
  printer. Nothing is displayed to the user. The tool appears to
  simply take longer to execute.

---

## Signal Interrupts

If the user sends an interrupt (Ctrl+C) during tool execution, the
existing `CancellationToken` mechanism applies. Inquiry tasks receive
a child token from the `ToolCoordinator`'s cancellation token and are
cancelled when the user interrupts.

On cancellation:
- The inquiry task is dropped (no cleanup needed since it never
  modified the real stream).
- The tool is marked as cancelled in the `ToolCoordinator`.
- If the `ToolCallRequest` for the cancelled tool was already added
  to the stream by the turn coordinator (during the `Streaming`
  phase), the existing `sanitize_orphaned_tool_calls` method handles
  the orphaned request by injecting a synthetic error response.

No special interrupt handling is needed for inquiries beyond what
already exists for tool cancellation.

---

## Error Handling

### Provider Errors

The inquiry uses `resilient_stream::collect_with_retry`, which handles
transport errors (rate limits, timeouts) with automatic retries.

### Non-Retryable Errors

If the provider returns a non-retryable error, the inquiry task sends
`InquiryFailed` via the event channel. The `ToolCoordinator` marks the
tool as completed with an error response. There is no fallback to the
old re-invocation flow.

### Task Panics

If the inquiry task panics, the `tokio::spawn` handle is dropped and
the tool never receives an answer. The `ToolCoordinator`'s existing
timeout/completion logic handles this (the tool is eventually reported
as incomplete when `execute_with_prompting` returns).

---

## Provider Caching

Even though the inquiry makes a separate provider call, the
conversation prefix in the inquiry thread is identical to the one from
the main streaming call (same events, same system prompt, same
sections). Providers that cache conversation prefixes (e.g., Anthropic
prompt caching) benefit from this overlap. The only difference is the
last few events (the temporary `ToolCallResponse` and `ChatRequest`
appended by the inquiry).

---

## Token Efficiency

### Before (Current System)

Example: `fs_modify_file` with a 500-token pattern list.

```
Initial call:       500 tokens (arguments)
Question prompt:     50 tokens (system message)
Re-invocation:      550 tokens (all arguments + tool_answers)
Total:             1100 tokens
```

### After (Stateful Inquiries)

```
Initial call:          500 tokens (arguments)
Structured inquiry:     30 tokens (question + schema overhead)
Total:                 530 tokens
```

Savings scale with argument size. A tool with 5000-token arguments
saves ~5000 tokens per question.

---

## Testing Strategy

### Unit Tests (Phase 1 -- Completed)

Schema generation and answer extraction:

Location: `crates/jp_cli/src/cmd/query/tool/inquiry.rs`

```rust
#[test] fn test_create_inquiry_schema_boolean() { ... }
#[test] fn test_create_inquiry_schema_select() { ... }
#[test] fn test_create_inquiry_schema_text() { ... }
#[test] fn test_extract_answer_valid() { ... }
#[test] fn test_extract_answer_id_mismatch() { ... }
#[test] fn test_extract_answer_missing_field() { ... }
#[test] fn test_tool_call_inquiry_id() { ... }
```

### Unit Tests (Phase 2)

The `inquire` function with mock provider:

```rust
#[tokio::test] fn test_inquire_returns_answer() { ... }
#[tokio::test] fn test_inquire_propagates_error() { ... }
#[tokio::test] fn test_inquire_cancellation() { ... }
```

### Integration Tests (Phase 2)

ToolCoordinator with inquiry support:

```rust
#[tokio::test] fn test_tool_with_single_inquiry() { ... }
#[tokio::test] fn test_tool_with_multiple_inquiries() { ... }
#[tokio::test] fn test_parallel_tools_with_inquiries() { ... }
#[tokio::test] fn test_inquiry_cancellation() { ... }
#[tokio::test] fn test_inquiry_failure_marks_tool_as_error() { ... }
```

---

## Migration Path

### Phase 1: Inquiry Infrastructure (Completed)

Location: `crates/jp_cli/src/cmd/query/tool/inquiry.rs`

1. `ActiveInquiry` helper struct (schema + answer extraction)
2. `create_inquiry_schema` (generates JSON schema from question type)
3. `tool_call_inquiry_id` (deterministic ID from tool name + call ID)
4. `extract_answer` (validates and extracts answer from structured
   response)
5. `Question` and `AnswerType` derive `Clone` (in `jp_tool`)
6. Unit tests (18 tests passing)

### Phase 2: InquiryBackend Trait + LLM Implementation

1. Define `InquiryBackend` trait with `inquire` method.
2. Implement `LlmInquiryBackend` (holds `Arc<dyn Provider>`,
   `ModelDetails`, system prompt, sections, attachments).
3. Implement `MockInquiryBackend` for tests.
4. Unit tests for `LlmInquiryBackend` with mock provider.

No changes to `ToolCoordinator` or turn loop. Can be reviewed and
merged independently.

### Phase 3: ToolCoordinator Integration

1. Add `InquiryAnswer` and `InquiryFailed` variants to
   `ExecutionEvent`.
2. Add `spawn_inquiry` method to `ToolCoordinator` (mirrors
   `spawn_user_prompt`).
3. Add `inquiry_backend` and `inquiry_events` parameters to
   `execute_with_prompting` and `handle_tool_result`. When
   `NeedsInput` + `QuestionTarget::Assistant`, call
   `spawn_inquiry`. `handle_tool_result` stays synchronous
   (same pattern as `spawn_user_prompt`).
4. Handle `InquiryAnswer` inline in the event loop (insert
   answer, re-spawn tool).
5. Handle `InquiryFailed` in the event loop (mark tool as
   completed with error).
6. Update the turn loop call site to pass `MockInquiryBackend`
   as a placeholder. Tests use mocks, no real provider calls.
7. Integration tests with `MockInquiryBackend`.

Depends on Phase 2.

### Phase 4: Turn Loop Wiring

1. Convert `Box<dyn Provider>` from `get_provider` to
   `Arc<dyn Provider>` in the turn loop via `Arc::from`.
2. Construct `LlmInquiryBackend` once before the main loop.
3. Replace the `MockInquiryBackend` placeholder from Phase 3
   with `Arc::clone(&inquiry_backend)` and pass the real
   `&ConversationStream` to `execute_with_prompting` at each
   `Executing` phase.
4. End-to-end tests.

Depends on Phase 3. Smallest phase — replaces the mock backend
with the real `LlmInquiryBackend` at the call site.

### Phase 5: Cleanup (Completed)

1. Removed `format_llm_question_response` and its tests.
2. Removed `tool_answers` extraction from `accumulated_answers_for_tool`
   (now `static_answers_for_all_questions`), executor stripping, and
   renderer hiding.
3. Removed `pending_tool_call_questions` from `TurnState`.
4. Non-interactive user-targeted questions now route through the
   inquiry backend instead of the old LLM re-invocation path.
5. Consolidated `InquiryAnswer`/`InquiryFailed` into single
   `InquiryResult` variant.

---

## Tracing

Inquiry operations emit structured tracing events:

```rust
tracing::info!(
    inquiry_id,
    question_type = ?question.answer_type,
    question_text = %question.text,
    "Structured inquiry initiated"
);

tracing::info!(
    inquiry_id,
    answer = ?answer,
    duration_ms = %elapsed.as_millis(),
    "Structured inquiry completed"
);
```

Filter: `RUST_LOG=jp_cli::cmd::query=debug`

---

## Future Enhancements

### Non-Tool Inquiry Sources

The inquiry infrastructure is not specific to tool calls. Future uses
could include conversation disambiguation, confirmation prompts, or
missing information requests. These would use different ID formats but
the same `inquire` function.

### Batched Inquiries

If multiple tools need answers simultaneously, their questions could
be batched into a single structured request with a multi-property
schema. Not planned for the initial implementation.

---

## References

- [Structured Output Architecture](./structured-output.md) --
  Foundation for inquiry schemas
- [Query Stream Pipeline](./query-stream-pipeline.md) -- Turn loop
  and streaming architecture
- `jp_tool::Question`, `jp_tool::AnswerType` -- Tool question types
- `jp_conversation::event::ChatRequest` -- Schema field for
  structured output
- `jp_conversation::event::ChatResponse::Structured` -- Structured
  response variant
- `jp_llm::resilient_stream::collect_with_retry` -- Retry logic for
  background LLM requests
