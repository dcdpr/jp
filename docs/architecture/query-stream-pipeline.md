# Query Stream Pipeline Architecture

This document describes the target architecture for the `jp query` command's
stream handling pipeline. It addresses the architectural issues identified in
[architecture.md](architecture.md) and provides a blueprint for refactoring
toward better separation of concerns, testability, and maintainability.

## Table of Contents

- [Overview](#overview)
- [Design Goals](#design-goals)
- [Core Concepts](#core-concepts)
  - [Turns and Cycles](#turns-and-cycles)
  - [Event Model](#event-model)
  - [Existing Types](#existing-types)
- [Architecture Overview](#architecture-overview)
- [Component Details](#component-details)
  - [Resilient Cycle](#resilient-cycle)
  - [Turn Coordinator](#turn-coordinator)
  - [Event Builder](#event-builder)
  - [Chat Response Renderer](#chat-response-renderer)
  - [Tool Coordinator](#tool-coordinator)
  - [Tool Executor](#tool-executor)
  - [Tool Renderer](#tool-renderer)
  - [Interrupt Handler](#interrupt-handler)
- [State Machine](#state-machine)
- [Data Flow](#data-flow)
  - [Streaming Flow](#streaming-flow)
  - [Tool Execution Flow](#tool-execution-flow)
  - [Interrupt Flow](#interrupt-flow)
  - [Continue Flow](#continue-flow)
- [Rendering Architecture](#rendering-architecture)
- [Error Handling](#error-handling)
- [Testing Strategy](#testing-strategy)
- [Migration Path](#migration-path)

---

## Overview

The query stream pipeline handles the core interaction loop between the user,
LLM providers, and tools. It is responsible for:

1. Receiving streamed events from LLM providers
2. Rendering content to the terminal with low latency
3. Executing tool calls (with user prompts when needed)
4. Accumulating events for persistence
5. Managing conversation state across multiple request-response cycles
6. Handling interrupts (Ctrl+C) gracefully

The current implementation suffers from tight coupling, mixed concerns, and
difficult testability. This architecture introduces clear component boundaries,
a state machine for turn management, and separation between rendering and
persistence.

---

## Design Goals

| Goal | Description |
|------|-------------|
| **Low-latency rendering** | Display LLM output as soon as possible, using minimal buffering |
| **Separation of concerns** | Each component has a single responsibility |
| **Testability** | Components can be unit tested in isolation |
| **Order preservation** | Events are rendered and persisted in correct order |
| **Graceful interrupts** | Ctrl+C provides interactive options, not just abort |
| **Resilient execution** | Transient errors are retried without losing progress |
| **Parallel tool execution** | Multiple tools can execute concurrently |

---

## Core Concepts

### Turns and Cycles

A **turn** is the complete interaction initiated by a user query until the
assistant provides a final response. A turn consists of one or more **cycles**.

A **cycle** is a single request-response exchange with the LLM:

```
┌─────────────────────────────────────────────────────────────────────┐
│                              TURN                                   │
│                                                                     │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │ Cycle 1                                                     │    │
│  │                                                             │    │
│  │   User: "What is 2+2?"                                      │    │
│  │   Assistant: [reasoning] → [message] → [tool: calculator]   │    │
│  │                                                             │    │
│  └─────────────────────────────────────────────────────────────┘    │
│                              │                                      │
│                              │ tool call requires follow-up         │
│                              ▼                                      │
│  ┌─────────────────────────────────────────────────────────────┐    │
│  │ Cycle 2                                                     │    │
│  │                                                             │    │
│  │   [tool response: "4"]                                      │    │
│  │   Assistant: [reasoning] → [message: "The answer is 4"]     │    │
│  │                                                             │    │
│  └─────────────────────────────────────────────────────────────┘    │
│                              │                                      │
│                              │ no more tool calls                   │
│                              ▼                                      │
│                         TURN COMPLETE                               │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

**Turn rules:**

- A turn MUST be initiated by a single user `ChatRequest`
- A turn MUST be followed by `ChatResponse`(s) and/or `ToolCallRequest`(s) from
  the assistant
- For each `ToolCallRequest`, the user MUST return a `ToolCallResponse`
- The turn CONTINUES while the assistant responds with `ToolCallRequest`(s)
- The turn ENDS when the assistant responds with `ChatResponse`(s) only (no tool
  calls)

### Event Model

LLM providers stream events using our internal representation:

```rust
pub enum Event {
    /// A part of a completed event.
    Part {
        /// Index identifying which logical event this belongs to.
        /// Different indices = different events (reasoning, message, tool
        /// call).
        index: usize,

        /// The partial event content.
        event: ConversationEvent,
    },

    /// Flush all parts with the given index.
    /// After flush, parts are merged into a complete ConversationEvent.
    Flush {
        index: usize,
        metadata: IndexMap<String, Value>,
    },

    /// The response stream has finished.
    Finished(FinishReason),
}
```

**Key properties:**

1. **Index-based grouping**: Each `index` represents one logical event. Parts
   with the same index are accumulated together.

2. **Flush boundary**: A `Flush { index }` signals that all parts for that
   index are complete and should be merged into a single `ConversationEvent`.

3. **Ordering**: Indices are assigned in order. Flush events arrive in index
   order. This preserves the sequence of events.

4. **Tool calls are single-part**: The `ToolCallRequestAggregator` ensures tool
   call requests are delivered as complete, single-part events (never chunked).

**Example stream (single cycle):**

```
Part { index: 0, ChatResponse::Reasoning("Let ") }
Part { index: 0, ChatResponse::Reasoning("me think") }
Flush { index: 0 }                                      → Reasoning complete

Part { index: 1, ChatResponse::Message("The ") }
Part { index: 1, ChatResponse::Message("answer is") }
Flush { index: 1 }                                      → Message complete

Part { index: 2, ToolCallRequest(calculator) }
Flush { index: 2 }                                      → Tool call 1 complete

Part { index: 3, ToolCallRequest(database) }
Flush { index: 3 }                                      → Tool call 2 complete

Finished(Completed)                                     → CYCLE ENDS HERE
```

**Important:** When tool calls are present, the cycle ends with `Finished`. The
LLM cannot reason about tool results until we execute the tools and send back
`ToolCallResponse`s in a NEW cycle. The example above shows a single cycle
that ends with two pending tool calls.

**Interleaved content within a cycle:** While it is technically possible for an
LLM to interleave reasoning, message, and tool call content within a single
cycle (e.g., message chunks at index 0, reasoning at index 1, more message at
index 2), this does NOT mean the LLM is reasoning about tool results. Any
reasoning within a cycle happens BEFORE tool execution, not after. Reasoning
about tool results requires a follow-up cycle after we return `ToolCallResponse`s.

**Example of interleaved content (still single cycle):**

```
Part { index: 0, ChatResponse::Message("Here's what") }
Flush { index: 0 }                                      → Message block 1

Part { index: 1, ChatResponse::Reasoning("Hmm, I need") }
Flush { index: 1 }                                      → Reasoning (mid-response)

Part { index: 2, ChatResponse::Message("I found") }
Flush { index: 2 }                                      → Message block 2

Part { index: 3, ToolCallRequest(search) }
Flush { index: 3 }                                      → Tool call

Finished(Completed)                                     → CYCLE ENDS
```

In this example, the output order is: message → reasoning → message → tool call.
The index determines rendering and persistence order, not the event type.

### Existing Types

The architecture uses existing types from the codebase:

**`ConversationEvent`** (`jp_conversation::event`):
```rust
pub struct ConversationEvent {
    pub timestamp: UtcDateTime,
    pub kind: EventKind,
    pub metadata: Map<String, Value>,
}

pub enum EventKind {
    ChatRequest(ChatRequest),
    ChatResponse(ChatResponse),      // Reasoning or Message
    ToolCallRequest(ToolCallRequest),
    ToolCallResponse(ToolCallResponse),
    InquiryRequest(InquiryRequest),
    InquiryResponse(InquiryResponse),
}
```

**`ChatResponse`** variants:
```rust
pub enum ChatResponse {
    Reasoning { reasoning: String },
    Message { message: String },
}
```

**`ConversationStream`** (`jp_conversation::stream`):
```rust
pub struct ConversationStream {
    base_config: Arc<AppConfig>,
    events: Vec<InternalEvent>,  // ConfigDelta or ConversationEvent
    pub created_at: UtcDateTime,
}
```

**`Thread`** (`jp_conversation::thread`):
```rust
pub struct Thread {
    pub system_prompt: Option<String>,
    pub instructions: Vec<InstructionsConfig>,
    pub attachments: Vec<Attachment>,
    pub events: ConversationStream,
}
```

The pipeline builds `ConversationEvent` instances and pushes them to
`ConversationStream`. Persistence serializes `ConversationStream` to disk.

---

## Architecture Overview

```
┌─────────────────┐
│   LLM Provider  │
└────────┬────────┘
         │
         ▼
┌─────────────────────────────────────────────────────────────────────┐
│                        Turn Coordinator                             │
│                        (State Machine)                              │
│                                                                     │
│  States: Idle → Streaming → Evaluating → Executing → ...            │
│                     ↓                        ↓                      │
│               Interrupted ←──────────── Interrupted                 │
│                     ↓                                               │
│          Complete | Aborted | Replying | Continue                   │
│                                             ↓                       │
│                                        [Assistant]                  │
│                                         [Prefill]                   │
│                                                                     │
│  Owns: state transitions, output ordering, cycle management         │
│                                                                     │
│  Uses Resilient Cycle wrapper for each LLM request                  │
│                                                                     │
└────────┬────────────────────────────────────────────────────────────┘
         │
         │ delegates to:
         │
    ┌────┴──────────────┬────────────────────┬──────────────────┐
    │                   │                    │                  │
    ▼                   ▼                    ▼                  ▼
┌──────────────┐ ┌──────────────┐ ┌─────────────┐ ┌────────────┐
│ Chat         │ │    Tool      │ │   Event     │ │ Interrupt  │
│ Response     │ │ Coordinator  │ │   Builder   │ │ Handler    │
│ Renderer     │ │              │ │             │ │            │
│              │ │ Manages      │ │ Buffers     │ │ Ctrl+C     │
│ Buffer       │ │ parallel     │ │ chunks      │ │ menus      │
│ Format       │ │ executors    │ │ by index    │ │            │
│              │ │              │ │             │ │ Context-   │
│              │ │ Orders       │ │ On flush:   │ │ aware      │
│              │ │ responses    │ │ pushes to   │ │ (stream vs │
│              │ │ for LLM      │ │ stream      │ │ tool)      │
└────┬─────────┘ └──────┬───────┘ └──────┬──────┘ └────────────┘
     │                  │                │
     │                  │                │
     │                  ▼                │
     │          ┌──────────────┐         │
     │          │ Tool         │         │
     │          │ Renderer     │         │
     │          └──────┬───────┘         │
     │                 │                 │
     └───────┬─────────┘                 │
             │                           │
             ▼                           │
     ┌──────────────┐                    │
     │   Printer    │                    │
     └──────────────┘                    │
                                         │
                                         │ push events
                                         ▼
                             ┌───────────────────────┐
                             │  ConversationStream   │
                             │  (inside Thread)      │
                             └───────────┬───────────┘
                                         │
                                         │ on cycle end
                                         ▼
                             ┌───────────────────────┐
                             │     Persistence       │
                             │     (Workspace)       │
                             └───────────────────────┘
```

**Note on Resilient Request:** The Turn Coordinator uses a `ResilientRequest`
wrapper internally when making LLM requests. Since a turn can span multiple
cycles (when tool calls are involved), the resilient wrapper is applied
per-cycle, not per-turn. If cycle N fails after cycles 1..(N-1) succeeded,
only cycle N is retried — previous cycles are already persisted.

---

## Component Details

### Resilient Cycle

Wraps a single request-response cycle with retry logic for transient errors.

**Scope:** Per-cycle, not per-turn. If cycle 100 fails, cycles 1-99 are already
persisted and unaffected.

**Handles:**
- Rate limits (429) — retry with backoff
- Timeouts — retry N times
- Empty responses — retry with modified prompt
- Transient network errors — retry

**Propagates (does not retry):**
- Authentication errors
- Unknown model errors
- Budget/quota exhausted
- Malformed requests

**Pseudo-code:**

```
fn resilient_request(request, max_retries = 3):
    for attempt in 1..=max_retries:
        result = provider.chat_completion_stream(request)

        match result:
            Ok(stream) =>
                response = consume_stream(stream)
                if response.is_empty() and attempt < max_retries:
                    request = append_retry_hint(request)
                    continue
                return Ok(response)

            Err(RateLimit { retry_after }) =>
                sleep(retry_after.unwrap_or(exponential_backoff(attempt)))
                continue

            Err(Timeout) if attempt < max_retries =>
                continue

            Err(e) =>
                return Err(e)

    return Err(MaxRetriesExceeded)
```

### Turn Coordinator

The central orchestrator implementing a state machine for turn management.

**Responsibilities:**
- Drive state transitions based on events and signals
- Route chunks to appropriate handlers (renderer, builder)
- Manage request-response cycles (loop on tool calls)
- Control output ordering
- Trigger persistence at cycle boundaries

**Does NOT:**
- Execute tools (delegates to Tool Coordinator)
- Format output (delegates to Renderers)
- Build events (delegates to Event Builder)
- Handle retry logic (delegates to Resilient Cycle)
- Persist state (delegates to Workspace)

**Interface:**

```
TurnCoordinator:
    fn start_turn(request: ChatRequest) -> TurnHandle
    fn handle_event(event: Event) -> Action
    fn handle_signal(signal: Signal) -> Action
    fn current_state() -> TurnState
```

**Actions returned:**

```
enum Action:
    Continue                    // Keep processing events
    RenderChunk(chunk)          // Send to renderer
    ExecuteTools(requests)      // Send to tool coordinator
    Persist                     // Flush to disk
    SendFollowUp(request)       // Start new cycle with tool responses
    Complete(result)            // Turn finished
    ShowInterruptMenu           // User pressed Ctrl+C
```

### Event Builder

Accumulates streamed chunks into complete `ConversationEvent` instances.

**Key insight:** Uses index-based buffering. Each index gets its own buffer.
On `Flush { index }`, the buffer for that index is finalized and pushed to
`ConversationStream`.

**State:**

```
struct EventBuilder:
    // Buffers keyed by event index
    buffers: HashMap<usize, IndexBuffer>

    // Reference to the conversation stream
    stream: &mut ConversationStream

enum IndexBuffer:
    Reasoning { content: String }
    Message { content: String }
    ToolCall { request: ToolCallRequest }
```

**Pseudo-code:**

```
fn handle_part(index, event):
    match event.kind:
        ChatResponse::Reasoning(r) =>
            buffers.entry(index)
                .or_insert(IndexBuffer::Reasoning(""))
                .append(r)

        ChatResponse::Message(m) =>
            buffers.entry(index)
                .or_insert(IndexBuffer::Message(""))
                .append(m)

        ToolCallRequest(tc) =>
            // Tool calls are always single-part, never appended
            buffers.insert(index, IndexBuffer::ToolCall(tc))

fn handle_flush(index, metadata):
    buffer = buffers.remove(index)

    event = match buffer:
        Reasoning { content } =>
            ConversationEvent::now(ChatResponse::Reasoning(content))
        Message { content } =>
            ConversationEvent::now(ChatResponse::Message(content))
        ToolCall { request } =>
            ConversationEvent::now(request)

    event.metadata.extend(metadata)
    stream.push(event)

fn handle_tool_response(response):
    // Tool responses come from Tool Executor, not LLM stream
    stream.add_tool_call_response(response)
```

**Properties:**
- One index = one event type (never mixes reasoning and message)
- Flush order matches index order (preserves sequence)
- Tool calls are single-part (guaranteed by `ToolCallRequestAggregator`)

### Chat Response Renderer

Renders `ChatResponse` events (reasoning and message content) to the terminal
with minimal latency.

**Scope:** This renderer is specifically for `ChatResponse` events from the LLM.
It does NOT handle `ChatRequest` (user messages), `ToolCallRequest`, or other
event types. The name explicitly reflects this limitation.

**Components:**

1. **Buffer** (`jp_md::buffer::Buffer`): Accumulates raw string chunks until
   a valid markdown block is formed. Emits blocks as soon as possible.

2. **Formatter** (`jp_md::format::Formatter`): Applies terminal formatting
   (ANSI codes for bold, italic, code, etc.) to markdown blocks.

3. **Display Mode Handler**: Applies display configuration (e.g., reasoning
   hidden, truncated, or full).

**Data flow:**

```
ChatResponse          Valid markdown blocks        Formatted output
    │                         │                          │
    ▼                         ▼                          ▼
┌────────┐               ┌───────────┐            ┌──────────┐
│ Buffer │ ────────────▶ │ Formatter │ ─────────▶ │ Printer  │
└────────┘               └───────────┘            └──────────┘
    │
    │ Example:
    │
    │ Input: ChatResponse::Message("# Hello")
    │        → (wait) →
    │        ChatResponse::Message(" World\n")
    │ Buffer emits: "# Hello World\n" (complete header)
    │ Formatter applies: bold, etc.
    │ Printer outputs with optional typewriter effect
```

**Pseudo-code:**

```
struct ChatResponseRenderer:
    buffer: jp_md::buffer::Buffer
    formatter: jp_md::format::Formatter
    printer: Printer
    config: StyleConfig
    last_was_reasoning: bool

fn render(response: ChatResponse):
    match response:
        ChatResponse::Reasoning { reasoning } =>
            render_reasoning(reasoning)

        ChatResponse::Message { message } =>
            render_message(message)

fn render_reasoning(content: &str):
    // Apply reasoning display mode
    match config.reasoning_display:
        Hidden =>
            return  // Don't render (still accumulated in Event Builder)

        Full =>
            render_content(content, is_reasoning: true)

        Truncate(max_chars) =>
            // Track total rendered, stop at max
            render_content(content.truncate(remaining), is_reasoning: true)

        Progress =>
            if !last_was_reasoning:
                printer.print("reasoning...")
            else:
                printer.print(".")

        Static =>
            if !last_was_reasoning:
                printer.print("reasoning...")

    last_was_reasoning = true

fn render_message(content: &str):
    // Insert separator if transitioning from reasoning
    if last_was_reasoning:
        printer.print("\n---\n\n")
        last_was_reasoning = false

    render_content(content, is_reasoning: false)

fn render_content(content: &str, is_reasoning: bool):
    // Feed to markdown buffer
    buffer.push(content)

    // Emit any complete blocks
    for block in buffer:
        formatted = formatter.format_terminal(block)
        delay = if is_code_block(block):
            config.typewriter.code_delay
        else:
            config.typewriter.text_delay
        printer.print_with_delay(formatted, delay)

fn flush():
    if remaining = buffer.flush():
        formatted = formatter.format_terminal(remaining)
        printer.print(formatted)
```

**Why `ChatResponse` input instead of `&str`:**

By accepting `ChatResponse` directly, the renderer:
1. Has explicit type information (reasoning vs message) without extra flags
2. Can apply variant-specific display logic internally
3. Makes the API self-documenting — callers know exactly what this renders
4. Future-proofs for potential new `ChatResponse` variants

### Tool Coordinator

Manages parallel execution of multiple tool calls while preserving order for
LLM responses.

**Responsibilities:**
- Spawn Tool Executors for each tool call (can be parallel)
- Collect responses and reorder to match request order
- Surface input prompts immediately (don't wait for order)
- Buffer render output for ordered emission (optional, see below)
- Manage cancellation token for all executors

**Key insight:** Input prompts and render output CAN be out of order. Only the
responses sent to the LLM MUST be in order.

**State:**

```
struct ToolCoordinator:
    executors: HashMap<CallId, ToolExecutor>
    pending_requests: Vec<ToolCallRequest>  // In order
    completed_responses: HashMap<CallId, ToolCallResponse>
    cancellation_token: CancellationToken   // Parent token for all executors

enum ExecutorState:
    Pending                         // Not yet started
    AwaitingInput(RunMode)          // Pre-execution: asking how to run the tool
    Running                         // Tool is executing
    AwaitingToolInput(Question)     // Mid-execution: tool needs user input
    AwaitingInput(ResultMode)       // Post-execution: asking how to handle result
    Completed(ToolCallResponse)     // Done
    Cancelled                       // User cancelled
```

**Note:** `AwaitingInput(RunMode)` replaces the simpler "permission prompt"
concept. The `RunMode` determines not just whether to run, but HOW to run:
- `Ask` — prompt user each time
- `Unattended` — run without prompts
- `Edit` — let user edit arguments before running
- `Skip` — skip execution entirely

Similarly, `AwaitingInput(ResultMode)` controls what happens after execution:
- `Ask` — prompt user before delivering result to LLM
- `Unattended` — deliver result as-is
- `Edit` — let user edit the result
- `Skip` — don't deliver result to LLM

**Pseudo-code:**

```
fn execute_all(requests: Vec<ToolCallRequest>) -> Vec<ToolCallResponse>:
    // Store request order
    pending_requests = requests.clone()

    // Spawn executors (can be parallel with async)
    for request in requests:
        executor = ToolExecutor::new(request)
        executors.insert(request.id, executor)
        spawn(executor.run())

    // Process executor events as they arrive
    while !all_completed():
        event = await next_executor_event()

        match event:
            NeedsPermission { id, prompt } =>
                // Show prompt immediately (out of order OK)
                tool_renderer.render_permission_prompt(prompt)
                answer = await user_input()
                executors[id].provide_permission(answer)

            NeedsInput { id, question } =>
                // Show prompt immediately (out of order OK)
                tool_renderer.render_input_prompt(question)
                answer = await user_input()
                executors[id].provide_input(answer)

            RenderOutput { id, output } =>
                // Can render out of order
                tool_renderer.render_output(output)

            Completed { id, response } =>
                completed_responses.insert(id, response)

    // Return responses in original request order
    return pending_requests.iter()
        .map(|r| completed_responses[r.id])
        .collect()
```

### Tool Executor

Executes a single tool call, handling the full lifecycle including prompts.

**Lifecycle:**

```
┌─────────────────────────────────────┐
│  Pre-execution: RunMode Prompt      │
│  (state: AwaitingInput(RunMode))    │
│                                     │
│  "Run tool X with args Y?"          │
│  Options:                           │
│  [y] Run (unattended)               │
│  [n] Skip                           │
│  [e] Edit arguments first           │
│  [r] Change run mode                │
│  [x] Change result mode             │
│  [p] Print raw arguments            │
└───────────────┬─────────────────────┘
                │
       ┌────────┼────────┬────────┐
       │        │        │        │
       │ run    │ skip   │ edit   │
       ▼        ▼        ▼        │
┌─────────────┐ ┌──────────────┐  │
│  Execute    │ │ToolCallResp │  │
│  Tool       │ │(skipped)    │  │
│             │ └──────────────┘  │
│ (state:     │                   │
│  Running)   │◀──────────────────┘
└──────┬──────┘   (after editing args)
       │
       │ (tool may request input during execution)
       ▼
┌─────────────────────────────────────┐
│  Mid-execution: Tool Input Prompts  │
│  (state: AwaitingToolInput)         │
│                                     │
│  Tool asks: "Which branch?"         │
│  [user provides input]              │
│  (may repeat multiple times)        │
└───────────────┬─────────────────────┘
       │
       │ (tool completes)
       ▼
┌─────────────────────────────────────┐
│  Post-execution: ResultMode Prompt  │
│  (state: AwaitingInput(ResultMode)) │
│                                     │
│  "Tool returned: <result>"          │
│  Options:                           │
│  [y] Deliver to LLM                 │
│  [n] Don't deliver                  │
│  [e] Edit result first              │
└───────────────┬─────────────────────┘
       │
       ▼
┌─────────────────────────────────────┐
│  Final ToolCallResponse             │
│  (state: Completed)                 │
│  (sent to LLM via Tool Coordinator) │
└─────────────────────────────────────┘
```

**Interface:**

```
ToolExecutor:
    fn new(request: ToolCallRequest, config: ToolConfig) -> Self
    async fn run() -> ToolCallResponse

    // Pre-execution configuration
    fn configure_run_mode(mode: RunMode)
    fn provide_edited_arguments(args: Value)

    // Mid-execution input
    fn provide_tool_input(question_id: &str, answer: Value)

    // Post-execution configuration
    fn configure_result_mode(mode: ResultMode)
    fn provide_edited_result(result: String)

    fn state() -> ExecutorState
```

**RunMode options** (see `jp_llm::tool`):
- `Ask` — prompt user before execution (default for interactive)
- `Unattended` — execute without prompts
- `Edit` — open editor to modify arguments before execution
- `Skip` — skip execution, return "skipped" response

**ResultMode options**:
- `Ask` — prompt user before delivering result
- `Unattended` — deliver result as-is (default)
- `Edit` — open editor to modify result before delivery
- `Skip` — don't deliver result, return "success" placeholder

### Tool Renderer

Formats tool-related output for the terminal. "Dumb" renderer — only formats
what it's told, doesn't make decisions.

**Input types:**

```
enum ToolRenderCommand:
    ShowCallStart { name: String, args: FormattedArgs }
    ShowPermissionPrompt { question: String, options: Vec<char> }
    ShowInputPrompt { question: Question }
    ShowProgress { elapsed: Duration }  // For long-running tools
    ShowResult { content: String, truncated: bool }
    ShowError { message: String }
    ShowLink { path: PathBuf, style: LinkStyle }
```

**Pseudo-code:**

```
fn render(command: ToolRenderCommand):
    match command:
        ShowCallStart { name, args } =>
            printer.print(format!("\nCalling tool {name}"))
            printer.print(format_args(args))
            printer.print("\n\n")

        ShowPermissionPrompt { question, options } =>
            // Delegate to inquire or custom prompt
            show_inline_select(question, options)

        ShowProgress { elapsed } =>
            // Use ANSI escape to update in place
            printer.print(format!("\r⏱ Running... {elapsed}"))

        ShowResult { content, truncated } =>
            if truncated:
                printer.print(format!("\nTool result (truncated):\n"))
            else:
                printer.print(format!("\nTool result:\n"))
            printer.print(format_code_block(content))

        ShowLink { path, style } =>
            match style:
                Full => printer.print(format!("see: {path}"))
                Osc8 => printer.print(hyperlink(path, "open"))
                Off => ()
```

### Interrupt Handler

Manages Ctrl+C behavior with context-aware menus.

**Two contexts:**

1. **During Streaming**: Stream is paused, user chooses action
2. **During Tool Execution**: Tools can be cancelled via `CancellationToken`

**Streaming interrupt menu:**

```
┌─────────────────────────────────────┐
│  Interrupted during streaming       │
│                                     │
│  [s] Stop - save what we have       │
│  [a] Abort - discard, no save       │
│  [r] Reply - respond to LLM now     │
│  [c] Continue - resume streaming    │
│                                     │
└─────────────────────────────────────┘
```

**Tool execution interrupt menu:**

```
┌─────────────────────────────────────┐
│  Interrupted during tool execution  │
│                                     │
│  [s] Stop - cancel tool, reply      │
│  [r] Restart - cancel and retry     │
│  [c] Continue - wait for tool       │
│                                     │
└─────────────────────────────────────┘
```

**Pseudo-code:**

```
fn handle_interrupt(context: InterruptContext) -> InterruptAction:
    match context:
        Streaming { stream_alive, partial_content } =>
            choice = show_streaming_menu()
            match choice:
                's' => InterruptAction::Stop
                'a' => InterruptAction::Abort
                'r' => InterruptAction::Reply(get_user_reply())
                'c' =>
                    if stream_alive:
                        InterruptAction::Resume
                    else:
                        InterruptAction::Continue { partial_content }

        ToolExecution { tool_id, executor_state } =>
            choice = show_tool_menu()
            match choice:
                's' =>
                    // Trigger cancellation via token
                    // Executors will terminate at next check point
                    InterruptAction::ToolCancelled {
                        response: "Tool cancelled by user"
                    }
                'r' =>
                    InterruptAction::RestartTool { tool_id }
                'c' =>
                    InterruptAction::Resume
```

**Cancellation mechanism:**

Tools are cancelled using `tokio_util::sync::CancellationToken`:

1. Tool Coordinator creates a parent token when preparing executors
2. Each executor receives a child token
3. On interrupt, the parent token is cancelled
4. All child tokens propagate cancellation
5. Local tools: abort the `wait_with_output` task (orphans the process)
6. MCP tools: race `mcp_client.call_tool()` against `token.cancelled()`

```
ToolCoordinator:
    cancellation_token: CancellationToken  // parent

fn execute_all():
    for executor in executors:
        child_token = cancellation_token.child_token()
        spawn(executor.execute(child_token))

    // On Ctrl+C + "Stop":
    cancellation_token.cancel()  // All children notified

ToolExecutor (local):
    wait_handle = spawn(child.wait_with_output())
    select! {
        () = token.cancelled() => abort_handle.abort()
        output = wait_handle => process(output)
    }

ToolExecutor (MCP):
    select! {
        () = token.cancelled() => return Cancelled
        result = mcp_client.call_tool() => process(result)
    }
```

---

## State Machine

The Turn Coordinator implements this state machine:

```
                            ┌──────────────────┐
                            │      Idle        │
                            │                  │
                            │  No active turn  │
                            └────────┬─────────┘
                                     │
                                     │ start_turn(ChatRequest)
                                     ▼
                            ┌──────────────────┐
                  ┌────────▶│    Streaming     │◀────────┐
                  │         │                  │         │
                  │         │ Receiving chunks │         │
                  │         │ from LLM         │         │
                  │         └────────┬─────────┘         │
                  │                  │                   │
                  │                  │ Ctrl+C            │
                  │                  ▼                   │
                  │         ┌──────────────────┐         │
                  │         │   Interrupted    │         │
                  │         │   (Streaming)    │         │
                  │         └──┬───┬───┬───┬───┘         │
                  │            │   │   │   │             │
                  │    Stop    │   │   │   │ Continue    │
                  │      ┌────┬┘   │   │   └─────┐       │
                  │      │    │    │   │         │       │
                  │      │   Abort │   │ Reply   │       │
                  │      │    │    │   │   │     │       │
                  │      ▼    │    │   │   │     ▼       │
                  │  Complete │    │   │   │   Resume ───┘
                  │      │    │    │   │   │     or
                  │      │    ▼    │   │   │   Prefill+Resume
                  │      │  Aborted│   │   │
                  │      │         │   │   │
                  │      │         │   │   ▼
                  │      │         │   │ ┌──────────────┐
                  │      │         │   │ │  Replying    │
                  │      │         │   │ │              │
                  │      │         │   │ │ User input   │
                  │      │         │   │ │ sent to LLM  │
                  │      │         │   │ └──────┬───────┘
                  │      │         │   │        │
                  │      │         │   │        └────────┐
                  │      └─────┐   │   │                 │
                  │            │   │   │                 │
                  │                                      │
                  │          (back to Streaming)         │
                  │                │                     │
                  │                │ Finished            │
                  │                ▼                     │
                  │       ┌──────────────────┐           │
                  │       │   Evaluating     │           │
                  │       │                  │           │
                  │       │ Any pending      │           │
                  │       │ tool calls?      │           │
                  │       └───┬─────────┬────┘           │
                  │           │         │                │
                  │      yes  │         │ no             │
                  │           ▼         │                │
                  │  ┌──────────────┐   │                │
                  │  │  Executing   │   │                │
                  │  │    Tools     │   │                │
                  │  │              │   │                │
                  │  │ Tool calls   │   │                │
                  │  │ in progress  │   │                │
                  │  └──────┬───────┘   │                │
                  │         │           │                │
                  │         │ Ctrl+C    │                │
                  │         ▼           │                │
                  │  ┌──────────────┐   │                │
                  │  │ Interrupted  │   │                │
                  │  │ (Tool)       │   │                │
                  │  └──┬───┬───┬───┘   │                │
                  │     │   │   │       │                │
                  │ Stop│ Restart│Continue               │
                  │     │   │   │       │                │
                  │     │   │   └───────┼───▶ Resume     │
                  │     │   │           │      execution │
                  │     │   └───────────┼───▶ Restart    │
                  │     │               │      tool      │
                  │     └───────────────┼───▶ Cancel,    │
                  │                     │      continue  │
                  │                     │                │
                  │         │           │                │
                  │         │ all tools │                │
                  │         │ complete  │                │
                  │         ▼           │                │
                  │  ┌──────────────┐   │                │
                  │  │  Continuing  │   │                │
                  │  │              │   │                │
                  │  │ Send tool    │   │                │
                  │  │ responses    │   │                │
                  │  │ to LLM       │   │                │
                  │  └──────┬───────┘   │                │
                  │         │           │                │
                  └─────────┘           │                │
                                        ▼                │
                            ┌──────────────────┐         │
                            │    Complete      │◀────────┘
                            │                  │
                            │ Persist & exit   │
                            └──────────────────┘
```

**Key clarifications:**

1. **Persistence at cycle boundaries:** Persistence occurs at the end of EACH
   cycle, not just at turn end. After streaming completes and tools execute,
   we persist before continuing to the next cycle.

2. **Replying starts a NEW turn:** When user presses Ctrl+C and chooses "Reply",
   their input becomes a new `ChatRequest` that starts a fresh turn. The current
   partial turn is persisted first, then the CLI returns to Idle, then
   immediately starts a new turn with the user's reply.

3. **Continuing loops back to Streaming:** After tool responses are collected
   and persisted, we send them to the LLM and enter `Streaming` again for the
   next cycle within the same turn.

4. **"(back to Streaming)" in diagram:** This refers to the `Continuing` state
   transitioning back to `Streaming` after sending tool responses to the LLM.
   The turn continues with a new cycle.

**State transitions:**

| From | Event | To | Action |
|------|-------|-----|--------|
| Idle | start_turn | Streaming | Send ChatRequest to LLM |
| Streaming | Event::Part | Streaming | Forward to Renderer + Builder |
| Streaming | Event::Flush | Streaming | Finalize event in Builder |
| Streaming | Event::Finished | Evaluating | Check for tool calls |
| Streaming | Ctrl+C | Interrupted(Streaming) | Pause, show menu |
| Interrupted | "Stop" | Complete | Persist current cycle |
| Interrupted | "Abort" | Aborted | Discard, no persist, → Idle |
| Interrupted | "Reply" | Replying | Get user input |
| Interrupted | "Continue" | Streaming | Resume or Prefill+Resume |
| Replying | User input | Idle → Streaming | Persist partial, start NEW turn with reply |
| Evaluating | Has tool calls | Executing | Start Tool Coordinator |
| Evaluating | No tool calls | Complete | Persist final cycle, → Idle |
| Executing | All tools done | Continuing | Persist cycle, prepare follow-up |
| Executing | Ctrl+C | Interrupted(Tool) | Show tool menu |
| Continuing | — | Streaming | Send tool responses to LLM (new cycle) |
| Complete | — | Idle | Turn done |

---

## Data Flow

### Streaming Flow

```
LLM Provider
     │
     │ Event::Part { index: 0, ChatResponse::Reasoning("Let me") }
     ▼
Turn Coordinator (state: Streaming)
     │
     ├────────────────────────────────────┐
     │                                    │
     ▼                                    ▼
Markdown Renderer                    Event Builder
     │                                    │
     │ buffer.push("Let me")              │ buffers[0] = Reasoning("Let me")
     │ (no complete block yet)            │
     │                                    │
     ▼                                    ▼
(waiting for more)                   (waiting for flush)

─────────────────────────────────────────────────────────────────

LLM Provider
     │
     │ Event::Part { index: 0, ChatResponse::Reasoning(" think\n\n") }
     ▼
Turn Coordinator
     │
     ├────────────────────────────────────┐
     │                                    │
     ▼                                    ▼
Markdown Renderer                    Event Builder
     │                                    │
     │ buffer.push(" think\n\n")          │ buffers[0].append(" think\n")
     │ → emits "Let me think\n\n"         │
     │ → formatter → printer              │
     │                                    │
     ▼                                    ▼
Terminal: "Let me think"             (waiting for flush)

─────────────────────────────────────────────────────────────────

LLM Provider
     │
     │ Event::Flush { index: 0, metadata }
     ▼
Turn Coordinator
     │
     │ (renderer already emitted)
     │
     ▼
Event Builder
     │
     │ finalize buffers[0]
     │ stream.push(ConversationEvent {
     │     kind: ChatResponse::Reasoning("Let me think\n\n"),
     │     metadata: metadata
     │ })
     │
     ▼
ConversationStream: [Reasoning("Let me think\n\n")]
```

### Tool Execution Flow

```
LLM Provider
     │
     │ Event::Part { index: 2, ToolCallRequest(calculator) }
     │ Event::Flush { index: 2 }
     │ Event::Finished(Completed)
     ▼
Turn Coordinator (state: Streaming → Evaluating)
     │
     │ Has pending tool calls: [calculator]
     │
     ▼
Turn Coordinator (state: Executing)
     │
     ▼
Tool Coordinator
     │
     │ spawn executor for calculator
     │
     ▼
Tool Executor (calculator)
     │
     ├─── NeedsPermission ──▶ Tool Renderer ──▶ "Run calculator?" ──▶ Terminal
     │                                               │
     │◀─────────── provide_permission(true) ◀───── User: "y"
     │
     ├─── execute tool ──────────────────────────────────────────────▶ MCP/Local
     │
     │◀─────────── result: "4" ◀─────────────────────────────────────────┘
     │
     ├─── RenderOutput ─────▶ Tool Renderer ──▶ "Result: 4" ──▶ Terminal
     │
     └─── Completed(response) ──▶ Tool Coordinator
                                       │
                                       │ all executors done
                                       ▼
                               Tool Coordinator
                                       │
                                       │ responses in order: [R1]
                                       ▼
                               Turn Coordinator (state: Continuing)
                                       │
                                       │ stream.add_tool_call_response(R1)
                                       │
                                       ▼
                               ConversationStream: [..., ToolCallRequest, ToolCallResponse]
                                       │
                                       │ send follow-up request
                                       ▼
                               Turn Coordinator (state: Streaming)
                                       │
                                       │ (new cycle begins)
```

### Tool Cancellation Flow

How tool cancellation propagates through the system:

```
User presses Ctrl+C during tool execution
                      │
                      ▼
        ┌──────────────────────────┐
        │   Interrupt Handler      │
        │   Shows tool menu        │
        │   User selects [s] Stop  │
        └──────────┬───────────────┘
                   │
                   │ returns InterruptAction::ToolCancelled
                   ▼
        ┌──────────────────────────┐
        │  Turn Coordinator        │
        │  (in query.rs)           │
        │                          │
        │  cancellation_token      │
        │      .cancel()           │
        └──────────┬───────────────┘
                   │
                   │ parent token cancelled
                   │
           ┌───────┴────────┬──────────────┐
           │                │              │
           ▼                ▼              ▼
    ┌─────────────┐  ┌─────────────┐  ┌─────────────┐
    │ Executor 1  │  │ Executor 2  │  │ Executor 3  │
    │ (MCP)       │  │ (Local)     │  │ (Local)     │
    │             │  │             │  │             │
    │ child_token │  │ child_token │  │ child_token │
    │ .cancelled()│  │ .cancelled()│  │ .cancelled()│
    │ fires       │  │ fires       │  │ fires       │
    └─────┬───────┘  └─────┬───────┘  └─────┬───────┘
          │                │                │
          ▼                ▼                ▼
    ┌─────────────────────────────────────────────┐
    │ tokio::select! arms fire immediately        │
    │                                             │
    │ MCP: return "cancelled by user"             │
    │ Local: abort_handle.abort() + return msg   │
    └─────────────────┬───────────────────────────┘
                      │
                      │ all executors return quickly
                      ▼
        ┌──────────────────────────┐
        │   Tool Coordinator       │
        │   Collects responses     │
        │   (all say "cancelled")  │
        └──────────┬───────────────┘
                   │
                   │ responses in original order
                   ▼
        ┌──────────────────────────┐
        │   Turn Coordinator       │
        │   Adds responses to      │
        │   ConversationStream     │
        │   Transitions to:        │
        │   Continuing → Streaming │
        └──────────────────────────┘
```

**Key points:**

1. Cancellation is **cooperative** - tools must check the token
2. Local tools orphan the child process (acceptable trade-off)
3. MCP tools return immediately with a cancellation message
4. All tools return responses (never left dangling)
5. The LLM receives cancellation messages like any other tool result

### Interrupt Flow

```
Turn Coordinator (state: Streaming)
     │
     │ receiving chunks...
     │
     │◀──────────────────────────────── Ctrl+C signal
     │
     ▼
Turn Coordinator (state: Interrupted)
     │
     ▼
Interrupt Handler
     │
     │ context: Streaming { stream_alive: true, partial: "The answer" }
     │
     ▼
Terminal: ┌─────────────────────────────┐
          │ [s] Stop   [a] Abort        │
          │ [r] Reply  [c] Continue     │
          └─────────────────────────────┘
     │
     │◀──────────────────────────────── User: "c"
     │
     ▼
Interrupt Handler
     │
     │ stream_alive = check_stream()
     │
     ├─── stream alive ───▶ InterruptAction::Resume
     │                           │
     │                           ▼
     │                      Turn Coordinator (state: Streaming)
     │                           │
     │                           │ continue receiving from same stream
     │
     │
     │
     └─── stream dead ────▶ InterruptAction::Continue { partial }
                                 │
                                 ▼
                            Turn Coordinator
                                 │
                                 │ build continuation request with assistant prefill
                                 │ send [User: Query] -> [Assistant: Partial]
                                 │
                                 ▼
                            LLM Provider (new stream)
                                 │
                                 │ first chunk: "... is 42."
                                 │ (continues exactly from where it left off)
                                 │
                                 ▼
                            Event Builder
                                 │
                                 │ buffer continues accumulating
                                 │
                                 ▼
                            Turn Coordinator (state: Streaming)
                                 │
                                 │ continue normal processing
```

### Continue Flow

Detailed view of the assistant prefill process:

```
Before interrupt:
─────────────────
ConversationStream:
  [ChatRequest("What is 2+2?")]
  [ChatResponse::Reasoning("Let me think")]  ← flushed, complete

Event Builder buffers:
  buffers[1] = Message("The answer")  ← NOT flushed, partial

─────────────────────────────────────────────────────────────────

User chooses "Continue", stream is dead:
────────────────────────────────────────

1. Build continuation request with prefill:

   Thread for LLM:
     [ChatRequest("What is 2+2?")]
     [ChatResponse::Reasoning("Let me think")]
     [ChatResponse::Message("The answer")]      ← injected as prefill

2. Send to LLM, receive continuation:

   LLM responds: " is 4. Because 2+2=4."

3. Update Event Builder:

   buffers[1].append(" is 4. Because 2+2=4.")
   // Total buffer content: "The answer is 4. Because 2+2=4."

4. Continue processing:

   More chunks arrive, appended to buffers[1]
   Eventually flush arrives
   Complete event pushed to stream

─────────────────────────────────────────────────────────────────

Final ConversationStream (persisted):
────────────────────────────────────

  [ChatRequest("What is 2+2?")]
  [ChatResponse::Reasoning("Let me think")]
  [ChatResponse::Message("The answer is 4. Because 2+2=4. ...")]
```

---

## Rendering Architecture

### Dual-Path Processing

Every chunk flows through TWO parallel paths:

1. **Render Path**: For immediate display (low latency)
2. **Accumulation Path**: For persistence (complete events)

```
                    Turn Coordinator
                           │
                           │ ChatResponse chunk
                           │
              ┌────────────┴────────────┐
              │                         │
              ▼                         ▼
      Markdown Renderer           Event Builder
              │                         │
              │ minimal buffering       │ accumulate until flush
              │ (valid markdown only)   │
              │                         │
              ▼                         ▼
          Printer               ConversationStream
              │                         │
              ▼                         ▼
         Terminal                     Disk
```

**Key insight:** The Render Path uses `jp_md::buffer::Buffer` which buffers
only enough to form valid markdown blocks. The Event Builder buffers until
`Flush` arrives. These are independent — rendering doesn't wait for flush.

### Output Ordering

Output ordering is determined by the **index** on each event, NOT by event type.

**Important:** Reasoning is NOT always first. An LLM can send events in any
order within a cycle. For example:

```
index 0: Message("Here's what I found")
index 1: Reasoning("Let me think about this more")
index 2: Message("Based on my analysis")
index 3: ToolCallRequest(search)
```

In this case, the output order is: message → reasoning → message → tool call.

**The Turn Coordinator's role:**
- Forward events to renderers in the order they arrive (by index)
- Does NOT reorder events based on type
- Index order is preserved for both rendering and persistence

**Tool call ordering:**
- Tool-related output can be out of order between different tools (T2's result
  can display before T1's result)
- However, tool call RESPONSES sent to the LLM MUST be in request order
- The Tool Coordinator handles this reordering internally

### Display Configuration

Reasoning display modes (applied in Markdown Renderer):

| Mode | Behavior |
|------|----------|
| `Hidden` | Don't render reasoning (still persisted) |
| `Full` | Render all reasoning tokens |
| `Truncate(N)` | Render first N characters, then "..." |
| `Progress` | Show "reasoning..." then dots |
| `Static` | Show "reasoning..." once |
| `Summary` | (Future) Summarize reasoning via new (async) LLM request |

---

## Error Handling

### Error Categories

| Category | Examples | Handling |
|----------|----------|----------|
| **Retryable** | Rate limit, timeout, empty response | Resilient Cycle retries |
| **Fatal** | Auth error, unknown model, quota | Propagate to user |
| **Tool error** | Tool execution failed | Return error in ToolCallResponse |
| **User cancel** | Ctrl+C | Interrupt Handler |

### Resilient Cycle Behavior

```
Error                    Action
─────────────────────────────────────────────────────────────────
RateLimit(retry_after)   Sleep, retry
Timeout                  Retry (up to max_retries)
Empty response           Append hint, retry
Connection error         Retry (up to max_retries)
Auth error               Propagate immediately
Unknown model            Propagate immediately
Quota exhausted          Propagate immediately
```

### Tool Error Handling

Tool errors don't fail the turn. They're returned to the LLM:

```
Tool execution fails
        │
        ▼
ToolCallResponse {
    id: request.id,
    result: Err("Tool failed: <error message>")
}
        │
        ▼
LLM receives error, can respond appropriately
```

---

## Testing Strategy

### Unit Testing

Each component can be tested in isolation:

**Event Builder:**
```
test "accumulates reasoning chunks":
    builder = EventBuilder::new(mock_stream)
    builder.handle_part(0, Reasoning("Hello "))
    builder.handle_part(0, Reasoning("world"))
    builder.handle_flush(0, {})

    assert mock_stream.events == [
        ConversationEvent { kind: Reasoning("Hello world") }
    ]
```

**Markdown Renderer:**
```
test "buffers until valid markdown":
    renderer = MarkdownRenderer::new(mock_printer)
    renderer.render_chunk("# Hello")  // no output yet
    renderer.render_chunk(" World\n") // now emits

    assert mock_printer.output == "# Hello World\n"
```

**Turn Coordinator:**
```
test "transitions to Executing on tool call":
    coordinator = TurnCoordinator::new()
    coordinator.start_turn(request)
    coordinator.handle_event(Part { index: 0, Message("Hi") })
    coordinator.handle_event(Part { index: 1, ToolCallRequest(...) })
    coordinator.handle_event(Flush { index: 0 })
    coordinator.handle_event(Flush { index: 1 })
    coordinator.handle_event(Finished)

    assert coordinator.state() == Executing
```

**Tool Cancellation:**
```
test "tools cancelled via token complete quickly":
    // Executors with 10 second delays
    executors = vec![
        MockExecutor::new("slow", Duration::from_secs(10), "result")
    ]

    coordinator = ToolCoordinator::with_executors(executors)
    token = coordinator.cancellation_token()

    // Cancel after 50ms
    spawn(async { sleep(50ms); token.cancel() })

    start = now()
    responses = coordinator.execute_all(...)
    elapsed = start.elapsed()

    assert elapsed < 500ms  // Not 10 seconds
    assert responses[0].result == Ok("Cancelled")
```

### Integration Testing

Test component interactions with mock LLM responses:

```
test "complete turn with tool call":
    mock_llm = MockLlm::new(vec![
        Part { index: 0, Message("Let me check") },
        Flush { index: 0 },
        Part { index: 1, ToolCallRequest(calculator) },
        Flush { index: 1 },
        Finished,
    ])

    mock_tool = MockToolExecutor::new(|req| {
        ToolCallResponse { result: Ok("42") }
    })

    pipeline = Pipeline::new(mock_llm, mock_tool)
    result = pipeline.run(ChatRequest("What is 6*7?"))

    assert result.stream.events == [
        ChatRequest("What is 6*7?"),
        ChatResponse::Message("Let me check"),
        ToolCallRequest(calculator),
        ToolCallResponse { result: Ok("42") },
        // ... follow-up cycle events
    ]
```

### Property Testing

```
test "events are persisted in stream order":
    for events in arbitrary_event_sequences():
        builder = EventBuilder::new(stream)
        for event in events:
            builder.handle(event)

        // Verify order matches flush order
        assert stream.events.indices() == sorted(flush_indices)
```

---

## Migration Path

### Phase 1: Extract Event Builder

1. Create `EventBuilder` struct with index-based buffering
2. Move chunk accumulation logic from `StreamEventHandler`
3. Add unit tests for `EventBuilder`
4. Integrate with existing `handle_stream` (minimal changes)

### Phase 2: Extract Renderers

1. Create `MarkdownRenderer` wrapping `jp_md::buffer::Buffer`
2. Create `ToolRenderer` for tool-related output
3. Move rendering logic from `ResponseHandler` and `StreamEventHandler`
4. Add unit tests for renderers

### Phase 3: Introduce Turn Coordinator

1. Create `TurnCoordinator` state machine
2. Define state enum and transitions
3. Move orchestration logic from `Query::handle_stream`
4. Add unit tests for state transitions

### Phase 4: Extract Tool Coordinator

1. Create `ToolCoordinator` for parallel execution
2. Create `ToolExecutor` for single tool lifecycle
3. Move tool execution logic from `StreamEventHandler::handle_tool_call`
4. Add unit tests for tool execution

### Phase 5: Add Resilient Cycle

1. Create retry wrapper for LLM requests
2. Move retry logic from `handle_event` and `handle_stream`
3. Add unit tests for retry behavior

### Phase 6: Implement Interrupt Handler

1. Create `InterruptHandler` with context-aware menus
2. Integrate with Turn Coordinator state machine
3. Implement `Continue` flow using assistant prefill
4. Add integration tests for interrupt scenarios

### Phase 7: Cleanup

1. Remove old `StreamEventHandler`, `ResponseHandler`
2. Simplify `Query::run` to use new pipeline
3. Update documentation
4. Performance testing

---

## Summary

This architecture addresses the key issues in the current implementation:

| Issue | Solution |
|-------|----------|
| Mixed concerns in `handle_stream` | Separate components with single responsibilities |
| Hard to test | Each component testable in isolation |
| Tight coupling | Clear interfaces between components |
| Implicit state | Explicit state machine |
| Recursive async | Event-driven loop |
| Blocking tool execution | Parallel Tool Coordinator |
| Abrupt Ctrl+C | Interactive Interrupt Handler |

The migration can be done incrementally, with each phase adding tests and
maintaining backward compatibility until the final cleanup.
