# JP Query Architecture

This document describes the technical architecture of the `jp query` command,
the core component responsible for LLM interactions. The goal is to provide a
foundation for refactoring toward better maintainability, decoupling, and
testability.

## Table of Contents

- [Overview](#overview)
- [System Architecture](#system-architecture)
- [Component Hierarchy](#component-hierarchy)
- [Data Flow](#data-flow)
- [Core Components](#core-components)
- [State Management](#state-management)
- [Error Handling](#error-handling)
- [Dependencies & Coupling](#dependencies--coupling)
- [Testing Challenges](#testing-challenges)
- [Refactoring Opportunities](#refactoring-opportunities)

## Overview

The `jp query` command orchestrates interactions between the user, LLM
providers, and tools via the Model Context Protocol (MCP). It handles:

- User input collection (CLI args, editor, templates)
- Conversation state management (workspace persistence)
- Configuration merging (files, env vars, CLI flags)
- Message thread construction (system prompts, history, attachments)
- LLM provider communication (streaming, structured output)
- Tool execution (local commands, MCP servers)
- Response rendering (Markdown, code blocks, streaming)

The command operates as a single-threaded async state machine with side
effects on disk (workspace persistence) and network (LLM/MCP calls).

## System Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                            CLI Entry Point                          │
│                          (jp_cli::main.rs)                          │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────────────┐
│                          Context Builder                            │
│  • Parse CLI args          • Load workspace    • Merge config       │
│  • Create runtime          • Init MCP client   • Setup signals      │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────────────┐
│                        Query::run(ctx)                              │
│                     (cmd/query.rs:233)                              │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
        ┌──────────────────────┴──────────────────────┐
        │                                             │
        ▼                                             ▼
┌──────────────────┐                        ┌──────────────────┐
│  Input Phase     │                        │  Output Phase    │
│  • Editor        │                        │  • Streaming     │
│  • Templates     │                        │  • Buffering     │
│  • Attachments   │                        │  • Persistence   │
└────────┬─────────┘                        └────────▲─────────┘
         │                                           │
         ▼                                           │
┌────────────────────────────────────────────────────┴─────────┐
│                      Execution Loop                          │
│  ┌─────────────────────────────────────────────────────┐     │
│  │  1. Build Thread (history + message + attachments)  │     │
│  │  2. Query LLM Provider (streaming)                  │     │
│  │  3. Handle Events (content, reasoning, tool calls)  │     │
│  │  4. Execute Tools (if requested)                    │     │
│  │  5. Loop Until Complete (recursion on tool results) │     │
│  └─────────────────────────────────────────────────────┘     │
└──────────────────────────────────────────────────────────────┘
```

## Component Hierarchy

### Crate Structure

```
jp_cli/               Command-line interface
  ├─ cmd/query.rs     Query command implementation (1,246 LOC)
  │   ├─ event.rs     Stream event handling (tool calls, chunks)
  │   ├─ turn.rs      Turn state (tool answers, retry count)
  │   └─ response_handler.rs  Terminal output rendering
  └─ ctx.rs           Shared context (workspace, config, MCP)

jp_llm/               LLM provider abstraction
  ├─ provider.rs      Provider trait + implementations
  │   ├─ anthropic.rs
  │   ├─ openai.rs
  │   ├─ google.rs
  │   ├─ ollama.rs
  │   └─ ...
  ├─ stream.rs        Streaming event types
  ├─ query/           Query builders (chat, structured)
  └─ tool.rs          Tool definition + execution

jp_conversation/      Conversation data structures
  ├─ conversation.rs  Conversation metadata
  ├─ message.rs       Message types (user, assistant, pairs)
  └─ thread.rs        Thread builder (system + history + message)

jp_workspace/         State persistence
  ├─ lib.rs           Workspace operations
  ├─ state.rs         In-memory state
  └─ query.rs         Conversation queries

jp_config/            Configuration system
  ├─ lib.rs           Config merging (files + env + CLI)
  ├─ assistant.rs     Assistant settings (model, params, tools)
  ├─ conversation.rs  Conversation settings (attachments, tools)
  └─ providers/       Provider-specific config

jp_mcp/               Model Context Protocol client
  └─ client.rs        MCP server lifecycle + tool calls

jp_task/              Background task handling
  └─ task/title_generator.rs  Async title generation
```

## Data Flow

### Query Execution Pipeline

```
1. CLI Input → 2. Config Merge → 3. Thread Build → 4. LLM Stream → 5. Persist

┌─────────┐
│  User   │
└────┬────┘
     │ jp query --new --model gpt-4 "explain async"
     ▼
┌────────────────────────────────────────┐
│  Parse & Validate                      │
│  • Flag parsing (clap)                 │
│  • Template rendering (Jinja2)         │
│  • Editor invocation (if no query)     │
└──────────────────┬─────────────────────┘
                   │ UserMessage::Query(String)
                   ▼
┌────────────────────────────────────────┐
│  Configuration Merge (Layered)         │
│  1. Default config                     │
│  2. Global config (~/.config/jp/)      │
│  3. Workspace config (.jp/config.toml) │
│  4. Conversation config (messages.json)│
│  5. Environment variables (JP_CFG_*)   │
│  6. CLI flags (--model, --tool, etc.)  │
└──────────────────┬─────────────────────┘
                   │ AppConfig
                   ▼
┌────────────────────────────────────────┐
│  Thread Construction                   │
│  • System prompt (from config)         │
│  • Instructions (tool usage, etc.)     │
│  • Attachments (files, resources)      │
│  • History (prior messages)            │
│  • Current message (user query)        │
└──────────────────┬─────────────────────┘
                   │ Thread
                   ▼
┌────────────────────────────────────────┐
│  Tool Definition Resolution            │
│  • Local tools (command execution)     │
│  • MCP tools (fetch from servers)      │
│  • Tool choice strategy (auto/manual)  │
└──────────────────┬─────────────────────┘
                   │ ChatQuery
                   ▼
┌────────────────────────────────────────┐
│  Provider Selection & Call             │
│  • Get provider (id → impl)            │
│  • Get model details (capabilities)    │
│  • Stream completion (event loop)      │
└──────────────────┬─────────────────────┘
                   │ EventStream
                   ▼
┌────────────────────────────────────────┐
│  Event Processing Loop                 │
│  ┌──────────────────────────────────┐  │
│  │  StreamEvent::ChatChunk          │  │
│  │  → ResponseHandler.handle()      │  │
│  │  → Stdout (streamed/buffered)    │  │
│  └──────────────────────────────────┘  │
│  ┌──────────────────────────────────┐  │
│  │  StreamEvent::ToolCall           │  │
│  │  → ToolDefinition.call()         │  │
│  │  → Execute (local/MCP)           │  │
│  │  → Collect result                │  │
│  │  → RECURSE (with tool results)   │  │
│  └──────────────────────────────────┘  │
│  ┌──────────────────────────────────┐  │
│  │  StreamEvent::EndOfStream        │  │
│  │  → Build MessagePair             │  │
│  └──────────────────────────────────┘  │
└──────────────────┬─────────────────────┘
                   │ Vec<MessagePair>
                   ▼
┌────────────────────────────────────────┐
│  State Persistence                     │
│  • Append messages to conversation     │
│  • Save to .jp/conversations/          │
│  • Update active conversation ID       │
│  • Trigger title generation (async)    │
└────────────────────────────────────────┘
```

### Stream Event Flow

```
Provider::chat_completion_stream() returns Pin<Box<dyn Stream<Item = Result<StreamEvent>>>>

                    StreamEvent
                         │
        ┌────────────────┼────────────────┐
        │                │                │
        ▼                ▼                ▼
  ChatChunk         ToolCall         Metadata
        │                │                │
   ┌────┴────┐           │                │
   │         │           │                │
Content  Reasoning       │                │
   │         │           │                │
   └────┬────┘           │                │
        │                │                │
        ▼                ▼                ▼
StreamEventHandler::handle_chat_chunk()
StreamEventHandler::handle_tool_call()
        │                │
        │                └─────────────────────┐
        │                                      │
        ▼                                      ▼
ResponseHandler.handle()              ToolDefinition.call()
        │                                      │
        ├─ Parse Markdown                      ├─ Local: duct::cmd()
        ├─ Syntax highlight                    └─ MCP: client.call_tool()
        ├─ Typewriter effect                          │
        └─ Stdout                                     ▼
                                           ToolCallResult
                                                  │
                                                  └─────────────┐
                                                                │
                                                                ▼
                                      RECURSE: handle_stream() with UserMessage::ToolCallResults
```

## Core Components

### 1. Query Command (`jp_cli::cmd::query`)

**Responsibilities:**
- CLI argument parsing and validation
- Message input collection (args, editor, templates)
- Configuration merging via `IntoPartialAppConfig` trait
- Orchestrating the entire query execution pipeline
- Error handling and cleanup

**State:**
- `Query` struct (CLI args only)
- `TurnState` (ephemeral per-turn state: retry count, tool answers)

**Key Methods:**
```rust
async fn run(self, ctx: &mut Ctx) -> Output
async fn build_message(&self, ctx: &mut Ctx, conversation_id: &ConversationId)
    -> Result<(UserMessage, Option<PathBuf>)>
async fn handle_stream(&self, ctx: &mut Ctx, turn_state: &mut TurnState,
    thread: Thread, tool_choice: ToolChoice, tools: Vec<ToolDefinition>,
    messages: &mut Vec<MessagePair>) -> Result<()>
```

**Problems:**
- 1,246 lines of code in a single file
- Deep nesting (6+ levels of indentation in `handle_stream`)
- Recursive async functions (Box::pin workaround)
- Mixes concerns (input, execution, output, persistence)
- Hard to test (requires full `Ctx` with real I/O)

### 2. Context (`jp_cli::ctx::Ctx`)

**Responsibilities:**
- Shared state container for the entire CLI session
- Owns workspace, config, MCP client, task handler, signals
- Provides immutable config access after initialization

**State:**
```rust
pub struct Ctx {
    pub workspace: Workspace,
    config: AppConfig,
    pub term: Term,
    pub mcp_client: jp_mcp::Client,
    pub task_handler: TaskHandler,
    pub signals: SignalPair,
    runtime: Runtime,
}
```

**Problems:**
- God object (all commands depend on it)
- No clear lifecycle (created once, mutated throughout)
- Mixes concerns (config, I/O, async runtime)
- Makes unit testing impossible (needs real workspace)

### 3. LLM Provider (`jp_llm::provider`)

**Responsibilities:**
- Abstract LLM provider API differences
- Stream response events (content, reasoning, tool calls, metadata)
- Handle provider-specific features (reasoning, structured output)

**Interface:**
```rust
#[async_trait]
pub trait Provider: Debug + Send + Sync {
    async fn model_details(&self, name: &Name) -> Result<ModelDetails>;
    async fn models(&self) -> Result<Vec<ModelDetails>>;
    async fn chat_completion_stream(&self, model: &ModelDetails,
        parameters: &ParametersConfig, query: ChatQuery) -> Result<EventStream>;
    async fn structured_completion(&self, model: &ModelDetails,
        parameters: &ParametersConfig, query: StructuredQuery) -> Result<Value>;
}
```

**Problems:**
- Trait cannot be mocked easily (async_trait)
- Provider selection is global (`get_provider(id, config)`)
- Error types are provider-specific but wrapped generically
- Reasoning extraction is post-hoc (parsed from stream)

### 4. Stream Event Handler (`jp_cli::cmd::query::event`)

**Responsibilities:**
- Accumulate streamed tokens into complete messages
- Distinguish reasoning from content tokens
- Execute tool calls (blocking async calls mid-stream)
- Prompt user for tool execution confirmation

**State:**
```rust
pub struct StreamEventHandler {
    pub reasoning_tokens: String,
    pub content_tokens: String,
    pub tool_calls: Vec<ToolCallRequest>,
    pub tool_call_results: Vec<ToolCallResult>,
}
```

**Problems:**
- Mutable state accumulated during streaming
- Blocks stream processing for tool execution
- User prompts during streaming (synchronous I/O)
- Turn-level state (persisted answers) lives elsewhere

### 5. Response Handler (`jp_cli::cmd::query::response_handler`)

**Responsibilities:**
- Render Markdown with syntax highlighting
- Apply typewriter effect for streaming
- Handle code block extraction and file saving
- Generate terminal hyperlinks (OSC 8)

**State:**
```rust
pub struct ResponseHandler {
    pub render_mode: RenderMode,
    pub render_tool_calls: bool,
    received: Vec<String>,
    pub parsed: Vec<String>,
    pub buffer: String,
    in_fenced_code_block: bool,
    code_buffer: (Option<String>, Vec<String>),
    // ...
}
```

**Problems:**
- Stateful line-by-line parsing (context-dependent)
- Mixes rendering logic with file I/O (saving code blocks)
- Handles both streaming and buffered modes differently
- Markdown parser state is implicit (`jp_md` integration)

### 6. Tool System (`jp_llm::tool`, `jp_tool`)

**Responsibilities:**
- Define tool schemas (parameters, descriptions)
- Execute tools (local commands, MCP calls)
- Handle tool prompts (confirmation, questions, editing)
- Return results to LLM

**Flow:**
```
ToolConfig → ToolDefinition → ToolDefinition.call() → ToolCallResult
                   ↓
           (Local: duct::cmd)
           (MCP: mcp_client.call_tool)
                   ↓
             Question/Answer
             (jp_tool::Question)
                   ↓
         inquire::prompt (blocking!)
```

**Problems:**
- Tool execution is synchronous and blocks event stream
- Local tool commands use shell execution (security concern)
- MCP tool parameters are dynamically merged (schema drift)
- Tool prompts (confirmation, editing) use blocking I/O

### 7. Workspace (`jp_workspace`)

**Responsibilities:**
- Manage conversation lifecycle
- Persist messages and metadata to disk
- Track active conversation

**Structure:**
```
.jp/
  ├─ conversations/
  │   ├─ <conversation-id>/
  │   │   ├─ metadata.json
  │   │   └─ messages.json
  ├─ state.json
  └─ config.toml
```

**Problems:**
- File I/O happens in `Drop` (implicit, error-prone)
- No transaction semantics (partial writes on crash)
- Active conversation is special-cased (stored separately)
- No versioning or migration strategy

## State Management

### Configuration Layers

Configuration merges happen in strict order, with later layers overriding
earlier ones:

```
1. Default (compiled into binary)
2. Global (~/.config/jp/config.toml)
3. Workspace (.jp/config.toml)
4. Extended (config.d/**/* glob)
5. Conversation (per-message, in messages.json)
6. Environment (JP_CFG_*)
7. CLI flags (--model, --tool, etc.)
```

**Implementation:**

```rust
// Merge happens via PartialConfig trait
let mut partial = PartialAppConfig::empty();
partial = load_file_config(partial, global_path)?;
partial = load_file_config(partial, workspace_path)?;
partial = load_extends(partial, extends_paths)?;
partial = load_conversation_config(partial, messages)?;
partial = PartialAppConfig::from_envs()?;
partial = Query::apply_cli_config(self, None, partial, Some(&partial))?;
let config = partial.finalize()?;
```

**Problems:**
- Order-dependent merging (no declarative precedence)
- Partial config is mutated in-place (hard to trace)
- CLI args are applied twice (once for validation, once for merge)
- No way to inspect "what config came from where"

### Conversation State

```rust
State {
    local: LocalState {
        active_conversation: Conversation,
        conversations: TombMap<ConversationId, Conversation>,
        messages: HashMap<ConversationId, Messages>,
    },
    user: UserState {
        conversations_metadata: ConversationsMetadata {
            active_conversation_id: ConversationId,
        },
    },
}
```

**Lifecycle:**
1. Load from disk on `Workspace::load()`
2. Mutate in-memory during command execution
3. Persist to disk on `Workspace::persist()` (explicit) or `Drop`

**Problems:**
- Active conversation is stored separately (prevents removal)
- No event sourcing (can't replay conversation)
- Metadata is split (Conversation vs Messages)
- Persistence is all-or-nothing (no incremental saves)

### Turn State

```rust
pub struct TurnState {
    pub persisted_tool_answers: IndexMap<String, IndexMap<String, Value>>,
    pub request_count: usize,
}
```

Ephemeral state for a single query turn. Tracks:
- Tool answers to reuse (e.g., "use this answer for all calls")
- Retry count (for rate limit handling)

**Problems:**
- Lives outside `Query` struct (passed as `&mut`)
- Cleared on recursion (nested tool calls lose state)
- No persistence across queries (user must re-answer)

## Error Handling

### Error Types

```
jp_cli::Error
  ├─ Llm(jp_llm::Error)
  ├─ Config(jp_config::Error)
  ├─ Workspace(jp_workspace::Error)
  ├─ Mcp(jp_mcp::Error)
  ├─ ToolError(jp_llm::ToolError)
  ├─ Io(std::io::Error)
  └─ Custom(String)
```

### Error Propagation

Errors bubble up with `?` operator, wrapped in crate-specific types. Most
errors terminate the command and print to stderr.

**Special Cases:**
- Rate limits: Retry with exponential backoff (handled in `handle_event`)
- Empty responses: Retry with modified prompt (handled in `handle_stream`)
- Tool errors: Return as `ToolCallResult` with `error: true`
- User cancellation: Signal handling via `SignalPair`

**Problems:**
- No error context (source location, trace)
- Mixed error types (some retryable, some fatal)
- Errors in `Drop` are printed to stderr (not propagated)
- Tool errors are swallowed and returned to LLM

## Dependencies & Coupling

### Dependency Graph

```
jp_cli
  ├─ jp_workspace
  │   ├─ jp_conversation
  │   ├─ jp_storage
  │   └─ jp_config
  ├─ jp_llm
  │   ├─ jp_conversation
  │   └─ jp_config
  ├─ jp_mcp
  │   └─ rmcp (external)
  ├─ jp_config
  │   └─ schematic (external)
  ├─ jp_task
  └─ jp_term
```

### Tight Coupling Points

**1. Query → Ctx (God Object)**
- Every operation requires `&mut Ctx`
- No interfaces, direct field access
- Cannot test without full workspace

**2. Config → Everything**
- Config is read by all components
- Deeply nested structure (config.assistant.model.parameters)
- No trait boundaries (concrete types everywhere)

**3. Tool Execution → MCP Client**
- Tool calls block event stream
- No abstraction over local vs MCP
- User prompts are synchronous

**4. Persistence → Workspace**
- Implicit on `Drop` (side effects)
- No dependency injection
- Cannot test without file I/O

### Circular Dependencies

None at the crate level, but conceptual cycles exist:

```
Query needs Config to build Thread
Config needs Query (for CLI overrides)
→ Solved via trait (IntoPartialAppConfig)

LLM needs Tools for schema
Tools need LLM for execution
→ Solved via indirection (ToolDefinition)
```

## Testing Challenges

### Current Test Coverage

- **Unit tests:** Minimal (mostly config parsing, serialization)
- **Integration tests:** None for query command
- **Property tests:** None

### Why Testing is Hard

**1. Global State Dependencies**

```rust
// Cannot construct without real workspace
async fn test_query() {
    let workspace = Workspace::new("/tmp/test").persisted()?;
    let config = AppConfig::default();
    let mut ctx = Ctx::new(workspace, runtime, args, config);

    // Now what? Need real LLM API keys...
}
```

**2. External I/O**

- File system (workspace persistence)
- Network (LLM APIs, MCP servers)
- Terminal (stdout, stdin for prompts)
- Editor (spawns external process)

**3. Async Complexity**

- Streaming requires tokio runtime
- Tool calls are nested async
- Signals and cancellation

**4. Non-deterministic Behavior**

- LLM responses vary
- Timestamps in conversation IDs
- Race conditions (background tasks)

**5. Tightly Coupled Code**

```rust
// Cannot mock because concrete types
let provider = provider::get_provider(id, &ctx.config().providers.llm)?;
let stream = provider.chat_completion_stream(&model, parameters, query).await?;

// Cannot inject test doubles
self.handle_stream(ctx, turn_state, thread, tool_choice, tools, messages).await?;
```

## Refactoring Opportunities

### 1. Extract Command Phases

Split `Query::run()` into discrete phases with clear boundaries:

```rust
pub struct QueryCommand {
    input: InputPhase,
    execution: ExecutionPhase,
    output: OutputPhase,
    persistence: PersistencePhase,
}

impl QueryCommand {
    pub async fn run(self, ctx: &Ctx) -> Result<Output> {
        let message = self.input.collect(ctx)?;
        let response = self.execution.execute(ctx, message).await?;
        self.output.render(ctx, &response)?;
        self.persistence.save(ctx, response)?;
        Ok(Output::Ok)
    }
}
```

**Benefits:**
- Each phase is testable in isolation
- Clear separation of concerns
- Can replace phases for testing (stub output, mock LLM)

### 2. Introduce Provider Abstraction

Replace direct provider calls with a trait object:

```rust
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn stream(&self, query: ChatQuery) -> Result<EventStream>;
}

pub struct RealLlmClient {
    provider: Box<dyn Provider>,
    model: ModelDetails,
}

pub struct MockLlmClient {
    responses: Vec<StreamEvent>,
}

// In tests
let client = Box::new(MockLlmClient::new(vec![
    StreamEvent::ChatChunk(CompletionChunk::Content("Hello".into())),
    StreamEvent::EndOfStream(StreamEndReason::Stop),
]));
```

**Benefits:**
- Can inject test doubles
- Provider selection is encapsulated
- Easier to add new providers

### 3. Decouple Tool Execution

Move tool execution out of event stream handling:

```rust
pub trait ToolExecutor: Send + Sync {
    async fn execute(&self, call: ToolCallRequest) -> Result<ToolCallResult>;
}

pub struct RealToolExecutor {
    definitions: HashMap<String, ToolDefinition>,
    mcp_client: Arc<jp_mcp::Client>,
}

pub struct MockToolExecutor {
    results: HashMap<String, ToolCallResult>,
}

// In event handler
let result = executor.execute(call).await?;
// No blocking prompts in stream
```

**Benefits:**
- Tools can be async without blocking stream
- Executor can be swapped (test, dry-run, parallel)
- User prompts can be decoupled (pre-approved, config-driven)

### 4. Make Context Injectable

Replace `Ctx` with focused traits:

```rust
pub trait ConfigProvider {
    fn config(&self) -> &AppConfig;
}

pub trait WorkspaceProvider {
    fn workspace(&self) -> &Workspace;
    fn workspace_mut(&mut self) -> &mut Workspace;
}

pub trait McpProvider {
    fn mcp_client(&self) -> &jp_mcp::Client;
}

// Query only needs what it uses
impl Query {
    async fn run<C, W, M>(
        self,
        config: &C,
        workspace: &mut W,
        mcp: &M,
    ) -> Result<Output>
    where
        C: ConfigProvider,
        W: WorkspaceProvider,
        M: McpProvider,
    {
        // ...
    }
}
```

**Benefits:**
- Clear dependencies (documented in trait bounds)
- Easier to mock (implement trait for test struct)
- Prevents leaking access to unrelated state

### 5. Event-Driven Architecture

Replace recursive `handle_stream` with event loop:

```rust
pub enum QueryEvent {
    StreamChunk(CompletionChunk),
    ToolCallRequested(ToolCallRequest),
    ToolCallCompleted(ToolCallResult),
    StreamEnded(StreamEndReason),
}

pub struct QueryStateMachine {
    state: QueryState,
    messages: Vec<MessagePair>,
}

impl QueryStateMachine {
    pub fn handle(&mut self, event: QueryEvent) -> Result<Action> {
        match (&self.state, event) {
            (QueryState::Streaming, QueryEvent::StreamChunk(chunk)) => {
                // Accumulate
                Ok(Action::Continue)
            }
            (QueryState::Streaming, QueryEvent::ToolCallRequested(call)) => {
                // Transition
                self.state = QueryState::ExecutingTool(call);
                Ok(Action::ExecuteTool)
            }
            // ...
        }
    }
}
```

**Benefits:**
- No recursion (easier to reason about)
- State machine is explicit (can visualize)
- Events can be recorded/replayed (debugging, testing)

### 6. Separate Rendering from Processing

Move `ResponseHandler` to a pure function:

```rust
pub fn render_markdown(
    input: &str,
    config: &StyleConfig,
    mode: RenderMode,
) -> Result<RenderedOutput> {
    // Pure function, no state
}

pub struct RenderedOutput {
    pub lines: Vec<String>,
    pub code_blocks: Vec<CodeBlock>,
    pub links: Vec<Hyperlink>,
}
```

**Benefits:**
- Testable without terminal
- Can render offline (e.g., in web UI)
- Easier to add new output formats (JSON, HTML)

### 7. Introduce Repository Pattern

Wrap workspace operations:

```rust
pub trait ConversationRepository {
    fn get(&self, id: &ConversationId) -> Option<&Conversation>;
    fn save(&mut self, id: ConversationId, conv: Conversation) -> Result<()>;
    fn delete(&mut self, id: &ConversationId) -> Result<()>;
    fn add_message(&mut self, id: ConversationId, msg: MessagePair) -> Result<()>;
}

pub struct WorkspaceRepository {
    workspace: Workspace,
}

pub struct InMemoryRepository {
    conversations: HashMap<ConversationId, Conversation>,
}
```

**Benefits:**
- Persistence logic is isolated
- Can swap storage (SQLite, cloud)
- Easy to test without disk I/O

### 8. Configuration Builder Pattern

Replace partial config mutation:

```rust
pub struct ConfigBuilder {
    layers: Vec<ConfigLayer>,
}

impl ConfigBuilder {
    pub fn add_file(mut self, path: &Path) -> Result<Self> {
        self.layers.push(ConfigLayer::File(path.into()));
        Ok(self)
    }

    pub fn add_env_vars(mut self) -> Result<Self> {
        self.layers.push(ConfigLayer::Env);
        Ok(self)
    }

    pub fn add_cli_args(mut self, args: CliArgs) -> Result<Self> {
        self.layers.push(ConfigLayer::Cli(args));
        Ok(self)
    }

    pub fn build(self) -> Result<AppConfig> {
        // Merge all layers in order
    }
}

// In tests
let config = ConfigBuilder::new()
    .add_cli_args(test_args)
    .build()?;
```

**Benefits:**
- Explicit layer ordering
- Can inspect "where did this value come from?"
- Easy to test specific merge scenarios

## Summary

The `jp query` command is the heart of the application, but suffers from:

**Architectural Issues:**
- God object pattern (`Ctx`)
- Deep nesting and tight coupling
- Mixed concerns (I/O, logic, rendering)
- Implicit dependencies (global state)

**Testing Issues:**
- Cannot unit test in isolation
- Requires real I/O (file, network, terminal)
- Non-deterministic (LLM, timestamps)
- Async complexity

**Maintainability Issues:**
- 1,246 LOC in one file
- Recursive async functions
- Stateful rendering (context-dependent)
- Implicit error handling

Refactoring toward testability requires:
1. Dependency injection (traits over concrete types)
2. Pure functions (side effects at boundaries)
3. Event-driven design (explicit state machine)
4. Repository pattern (storage abstraction)

This document serves as a map for incremental refactoring. Each opportunity
can be addressed independently, with tests added to prevent regressions.
