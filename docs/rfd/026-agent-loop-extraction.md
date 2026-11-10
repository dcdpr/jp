# RFD 026: Agent Loop Extraction

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-19
- **Required by**: [RFD 027]
- **Summary**: Extracts the agent turn loop from `jp_cli` into a standalone
  `jp_agent` crate with trait-based I/O hooks.

## Summary

This RFD extracts the turn loop and its supporting components from `jp_cli` into
a standalone `jp_agent` crate.
`jp_agent` provides an agent execution engine with trait-based I/O hooks,
separating core agent logic from CLI concerns such as terminal rendering,
interactive prompts, and process lifecycle.

## Motivation

The turn loop (`run_turn_loop`) and its supporting components
(`TurnCoordinator`, `ToolCoordinator`, `ChatResponseRenderer`,
`InterruptHandler`, `StreamRetryState`, and the inquiry backend) live inside
`jp_cli::cmd::query` with `pub(crate)` visibility.

This coupling creates two problems:

**The agent loop cannot be called from outside `jp_cli`.** Because these types
are `pub(crate)` inside the CLI crate, external consumers cannot run the turn
loop with custom output sinks, non-terminal prompt backends, or programmatic
signal injection.

**The boundary between agent execution and user interaction is implicit.** The
turn loop uses `TerminalPromptBackend`, `crate::error::Error`, and
`crate::signals::SignalTo` directly.
Although the `PromptBackend` trait provides a testing seam, the turn loop still
depends on CLI-specific signal and error types.

Extracting the agent loop defines an explicit API boundary: a function that runs
a conversation turn given a provider, tools, configuration, and I/O hooks.
The caller supplies those hooks, whether they write to a terminal, discard
output, or route I/O elsewhere.

> [!TIP]
> [RFD 027] is the primary consumer of `jp_agent`, using `run_turn_loop()` as
> the server-side execution engine in its client-server query architecture.

## Design

### What Moves

The following modules move from `jp_cli::cmd::query` into `jp_agent`:

| Current location       | New location                   | Role                       |
| ---------------------- | ------------------------------ | -------------------------- |
| `turn_loop.rs`         | `jp_agent::turn_loop`          | Main agent loop            |
| `turn/coordinator.rs`  | `jp_agent::turn::coordinator`  | Turn state machine         |
| `turn/state.rs`        | `jp_agent::turn::state`        | Per-turn accumulated state |
| `stream/retry.rs`      | `jp_agent::stream::retry`      | Stream retry logic         |
| `tool/coordinator.rs`  | `jp_agent::tool::coordinator`  | Parallel tool execution    |
| `tool/inquiry.rs`      | `jp_agent::tool::inquiry`      | LLM inquiry backend        |
| `interrupt/handler.rs` | `jp_agent::interrupt::handler` | Interrupt menu logic       |
| `interrupt/signals.rs` | `jp_agent::interrupt::signals` | Signal dispatch            |

### What Stays in `jp_cli`

Modules that handle CLI presentation and workspace integration remain in
`jp_cli`:

| Module                          | Why it stays                            |
| ------------------------------- | --------------------------------------- |
| `query.rs`                      | Argument parsing, config resolution,    |
|                                 | editor integration, and conversation    |
|                                 | selection.                              |
| `stream/renderer.rs`            | `ChatResponseRenderer`, which           |
|                                 | implements `ResponseRenderer` using     |
|                                 | `jp_md` for terminal presentation.      |
| `stream/structured_renderer.rs` | `StructuredRenderer`, which follows the |
|                                 | same `ResponseRenderer` pattern.        |
| `tool/executor.rs`              | `TerminalExecutorSource`, which creates |
|                                 | tool executors from workspace-resolved  |
|                                 | definitions.                            |
| `tool/prompter.rs`              | `ToolPrompter`, which renders           |
|                                 | permission and question prompts to the  |
|                                 | terminal behind the `PromptBackend`     |
|                                 | trait.                                  |
| `tool/renderer.rs`              | `ToolRenderer`, which renders tool call |
|                                 | progress and headers for the terminal.  |
| `tool/builtins.rs`              | Built-in tool registration (such as     |
|                                 | `describe_tools`).                      |

### Dependency Boundary

Most of the turn loop's external dependencies are already trait-based:

| Dependency      | Current type                      | Abstraction                            |
| --------------- | --------------------------------- | -------------------------------------- |
| LLM provider    | `Arc<dyn Provider>`               | Already a trait                        |
| Prompt backend  | `Arc<dyn PromptBackend>`          | Already a trait                        |
| Tool execution  | `Box<dyn ExecutorSource>`         | Already a trait                        |
| Inquiry backend | `Arc<dyn InquiryBackend>`         | Already a trait                        |
| Output          | `Arc<Printer>`                    | Concrete type, but writer-agnostic via |
|                 |                                   | `swap_writers`                         |
| Signals         | `SignalRx`                        | Concrete type                          |
|                 | (`broadcast::Receiver<SignalTo>`) |                                        |
| Persistence     | `&mut Workspace`                  | Concrete type                          |
| Error type      | `crate::error::Error`             | CLI-specific enum                      |

The remaining concrete dependencies are abstracted or moved as follows:

**`SignalTo`**: Defined in `jp_cli::signals` as a two-variant enum (`Shutdown`,
`Quit`).
It moves to `jp_agent` so the agent loop can react to signals regardless of
their source.

**`crate::error::Error`**: The turn loop returns `crate::error::Error`, which is
CLI-specific.
`jp_agent` defines its own `AgentError` type, and `jp_cli` wraps it.

**`Workspace`**: The turn loop calls `workspace.get_events()`,
`workspace.get_events_mut()`, and `workspace.persist_active_conversation()` to
read and persist conversation events.
Rather than depending on `jp_workspace` directly, `jp_agent` uses a
`ConversationStore` trait defined in `jp_conversation`:

```rust
pub trait ConversationStore {
    fn get_events(&self, id: &ConversationId) -> Option<&ConversationStream>;
    fn get_events_mut(&mut self, id: &ConversationId) -> Option<&mut ConversationStream>;
    fn persist(&mut self) -> Result<()>;
}
```

`jp_workspace::Workspace` implements `ConversationStore`, and other callers can
provide their own implementation.

**`ToolRenderer`** and **`ToolPrompter`**: Both stay concrete types constructed
inside the turn loop from its injected dependencies, rather than passed in via
new traits or generic parameters.

`ToolRenderer` renders progress indicators and tool call headers.
Rather than abstracting `ToolRenderer` behind a trait, the turn loop constructs
it from the `Arc<Printer>` it already accepts, and the printer's writers
determine where output goes.

`ToolPrompter` wraps `PromptBackend` with CLI formatting (colored permission
prompts and editor integration for result editing).
The turn loop constructs `ToolPrompter` from the `Arc<dyn PromptBackend>` it
already accepts; `jp_cli` passes `TerminalPromptBackend`, and other callers
provide their own implementation.

### `run_turn_loop` Signature

The extracted `run_turn_loop` function has the following signature:

```rust
pub async fn run_turn_loop(
    provider: Arc<dyn Provider>,
    model: &ModelDetails,
    cfg: &AppConfig,
    signals: &SignalRx,
    store: &mut dyn ConversationStore,
    conversation_id: ConversationId,
    printer: Arc<Printer>,
    prompt_backend: Arc<dyn PromptBackend>,
    inquiry_backend: Arc<dyn InquiryBackend>,
    tool_coordinator: ToolCoordinator,
    chat_request: ChatRequest,
    // Context that varies by caller
    root: &Utf8Path,
    is_tty: bool,
    mcp_client: &jp_mcp::Client,
    attachments: &[Attachment],
) -> Result<(), AgentError>
```

Three types change from the `jp_cli` signature:

- `&mut Workspace` becomes `&mut dyn ConversationStore`.
- `crate::error::Error` becomes `AgentError`.
- `SignalRx` and `SignalTo` move to `jp_agent`.

### `ToolRenderer` Stays Internal

`run_turn_loop` constructs `ToolRenderer` internally:

```rust
let mut tool_renderer = ToolRenderer::new(
    if cfg.style.tool_call.show && !printer.format().is_json() {
        printer.clone()
    } else {
        Printer::sink().into()
    },
    cfg.style.clone(),
    root.to_path_buf(),
    is_tty,
);
```

`ToolRenderer` moves to `jp_agent` alongside the turn loop, and this
construction stays inside `run_turn_loop`.
Because `ToolRenderer` writes through `Printer`, the caller controls its output
via writer injection.

### Response Rendering

`TurnCoordinator` owns a `ChatResponseRenderer` (which depends on `jp_md`) and
calls it on every streamed `ChatResponse` chunk.
The agent loop controls *when* rendering happens (on each chunk, flushing before
tool calls, and resetting on continuation), whereas *how* responses are rendered
is a presentation concern.

`jp_agent` defines a `ResponseRenderer` trait:

```rust
pub trait ResponseRenderer {
    /// Process a new response chunk from the LLM.
    fn render(&mut self, response: &ChatResponse);

    /// Emit any buffered content immediately.
    ///
    /// Called before tool calls (so buffered content appears before the
    /// tool output), on stream finish, and on interrupts.
    fn flush(&mut self);

    /// Discard all state, preparing for a fresh streaming cycle.
    ///
    /// Called when the turn loop restarts streaming after an interrupt
    /// (e.g. Continue or Reply action). The partial content has already
    /// been captured by the event builder.
    fn reset(&mut self);
}
```

Callers supply an implementation suited to their environment:

- A terminal renderer (`ChatResponseRenderer`) buffers Markdown, formats it with
  ANSI styling, and writes to the printer.
- A no-op renderer discards output during non-interactive execution.
- A test renderer captures events for assertions.

`TurnCoordinator` currently calls `reset_content_kind()` because the Markdown
renderer tracks transitions between reasoning and message content.
With `ResponseRenderer`, that bookkeeping moves inside `ChatResponseRenderer`:
the agent loop calls `flush()` when a tool call arrives, and the renderer
updates its own state.

`StructuredRenderer` follows the same pattern, either folded into
`ResponseRenderer` or defined as a parallel trait.
Keeping both renderers in `jp_cli` removes `jp_md` from `jp_agent`'s dependency
graph.

### Crate Dependencies

`jp_agent` depends on:

- `jp_config` (`AppConfig`, `StyleConfig`, and tool config types)
- `jp_conversation` (`ConversationStream`, events, `ConversationId`, and
  `ConversationStore`)
- `jp_llm` (`Provider`, `Event`, `ToolDefinition`, and `Executor`)
- `jp_printer` (`Printer`)
- `jp_inquire` (`PromptBackend`)
- `jp_mcp` (`Client`)
- `jp_attachment` (`Attachment`)
- `jp_tool` (`Question` and `AnswerType`)

`jp_cli` depends on `jp_agent` and provides:

- `TerminalPromptBackend` (via `jp_inquire`)
- `TerminalExecutorSource` (creates tool executors)
- `Workspace` implementing `ConversationStore`
- OS signal handling
- Argument parsing, editor integration, and config resolution

### Query Orchestration in `jp_cli`

After extraction, `query.rs` is the orchestration layer:

1. Parse arguments, resolve configuration, and select the conversation.
2. Open the editor and build the `ChatRequest`.
3. Resolve tools, set up MCP, and register attachments.
4. Create a `ToolCoordinator` with `TerminalExecutorSource`.
5. Create the `LlmInquiryBackend`.
6. Call `jp_agent::run_turn_loop(...)` with terminal-backed I/O.
7. Handle the result, persist the conversation, and clean up.

This matches what `Query::run()` does prior to extraction, without the turn loop
implementation itself.

## Drawbacks

**Workspace overhead.** Adding `jp_agent` introduces another node in the
workspace dependency graph and slightly increases build times.

**Parameter count.** `run_turn_loop` takes 16 parameters, and replacing `&mut
Workspace` with a persistence trait does not reduce that count.
Grouping parameters into a builder or context struct is left to a separate
refactoring.

> [!TIP]
> `TurnInputs` in `jp_cli::cmd::query` now groups the caller-specific inputs
> shown in `run_turn_loop` above and separates input collection (which reads CLI
> context) from turn execution (which does not).
> `TurnInputs::run` performs steps 3 through 5 of the orchestration sequence
> above in `jp_cli`, and `Query::run_turn` contains the turn-loop call that
> Phase 4 redirects to `jp_agent::run_turn_loop`.
> Because `Query::run_turn` already takes a `&ConversationLock` rather than
> `&mut Workspace`, `ConversationStore` has a smaller surface area than
> originally proposed here.

**Migration churn.** Moving modules across crates requires updating import paths
and adjusting tests that reference internal types.

## Alternatives

### Keep Renderers and Interrupt Handling in `jp_cli`

Move `run_turn_loop` and the coordinators into `jp_agent`, but leave
`ChatResponseRenderer`, `ToolRenderer`, and `InterruptHandler` in `jp_cli` and
pass them to the turn loop as trait objects or closures.

Rejected because only response rendering requires a trait boundary.
`ChatResponseRenderer` depends on `jp_md` for Markdown formatting, which is a
presentation concern that does not belong in `jp_agent`; the `ResponseRenderer`
trait (`render`, `flush`, `reset`) isolates that dependency.
`ToolRenderer` and `InterruptHandler` already operate through `Printer` and
`PromptBackend` and can move into `jp_agent` directly.

### Place the Agent Loop in `jp_llm`

Put the agent loop in `jp_llm`, which already hosts the provider traits and tool
execution types.

Rejected because `jp_llm` is a provider abstraction layer.
The agent loop depends on `jp_printer`, `jp_config`, and `jp_conversation`,
which do not belong in the LLM layer.
A separate crate keeps the dependency graph clean.

### Full Dependency Injection

Define traits for `Printer`, `ToolRenderer`, signal handling, and every other
concrete dependency to make the turn loop fully generic.

Rejected because the existing abstractions are sufficient.
`Printer` already supports writer injection, `ToolRenderer` is internal to the
loop rather than an extension point, and `SignalTo` is a simple enum.
Full dependency injection adds type parameters and complexity without enabling
use cases beyond what `ConversationStore`, `PromptBackend`, and
`ResponseRenderer` provide.

## Non-Goals

- **Public SDK.** `jp_agent` is an internal workspace crate without a stable or
  documented API for external consumers.
  A public SDK can be built on top of it later as a separate effort with its own
  design constraints.

- **Plugin system.** The trait-based hooks abstract I/O backends, not agent
  behavior.
  Hooks for tool filtering, response post-processing, or custom turn logic are
  out of scope.

- **Async trait migration.** Traits such as `InquiryBackend` and
  `ExecutorSource` continue to use `#[async_trait]`.
  This RFD does not change their signatures or migrate them to native async
  traits.

## Risks and Open Questions

### `ToolPrompter` and Editor Integration

`ToolPrompter` handles permission prompts and lets the user edit tool arguments
in `$EDITOR` before approving.
Opening `$EDITOR` is a CLI concern, so non-CLI callers need a different
mechanism for argument editing.

For the initial extraction, the turn loop constructs `ToolPrompter` from the
injected `PromptBackend`.
Supporting argument editing across backends may require extending
`PromptBackend` or introducing a dedicated trait for `ToolPrompter` during
implementation.

### MCP Client Dependency

The turn loop accepts `&jp_mcp::Client` and passes it through to
`ToolCoordinator` and `Executor` to run MCP-backed tools.
For now, `jp_mcp::Client` remains a concrete parameter; abstracting it behind a
trait is left for future work.

### `is_tty` Semantics

The turn loop uses `is_tty` to control progress indicators and `ToolRenderer`
behavior.
This flag describes the output consumer rather than the process.
For the initial extraction, `is_tty` stays a `bool` parameter; callers whose
output capabilities can change mid-turn will require a dynamic check instead.

## Implementation Plan

### Phase 1: Create `jp_agent` and Move Core Types

Create the `jp_agent` crate, define `AgentError`, and move `SignalTo` (with its
signal channel types) and `TurnState`, which have no dependencies on `jp_cli`
types.
Add the `ConversationStore` trait to `jp_conversation`.
`jp_cli` re-exports or wraps the moved types so existing tests continue to pass.
This phase can be merged independently.

### Phase 2: Move Turn Coordinator and Stream Retry

Move `TurnCoordinator` and `StreamRetryState` into `jp_agent`, and define the
`ResponseRenderer` trait.
Change `TurnCoordinator` to take `Box<dyn ResponseRenderer>` instead of owning
`ChatResponseRenderer` directly.
Keep `ChatResponseRenderer` and `StructuredRenderer` in `jp_cli`, implement
`ResponseRenderer` for them, and update `jp_cli` imports.
This phase can be merged independently.

### Phase 3: Move Tool Coordinator and Interrupt Handler

Move `ToolCoordinator` and `InterruptHandler` (the components that interact with
`PromptBackend` and signals at the I/O boundary) into `jp_agent`, and replace
`crate::Error` references in the moved code with `AgentError`.
This phase can be merged independently.

### Phase 4: Move the Turn Loop

Move `run_turn_loop` into `jp_agent` and replace `&mut Workspace` with `&mut dyn
ConversationStore`.
Update `jp_cli::cmd::query` to call `jp_agent::run_turn_loop(...)` with the
workspace as the store and terminal backends for I/O.
After this phase (which depends on Phases 1 through 3),
`jp_cli::cmd::query::run()` handles only CLI orchestration: config resolution,
conversation selection, and invoking the extracted agent loop.

## References

- `run_turn_loop` (`crates/jp_cli/src/cmd/query/turn_loop.rs`): Turn loop
  implementation extracted by this RFD.
- `PromptBackend` (`crates/jp_inquire/src/prompt.rs`): Trait for injecting
  interactive prompt I/O.
- `ExecutorSource` (`crates/jp_llm/src/tool/executor.rs`): Trait for creating
  tool executors.
- [yoagent]: Prior art for a standalone Rust agent loop library.

[RFD 027]: 027-client-server-query-architecture.md
[yoagent]: https://github.com/yologdev/yoagent
