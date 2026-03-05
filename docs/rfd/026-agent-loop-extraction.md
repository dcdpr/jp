# RFD 026: Agent Loop Extraction

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-19

## Summary

This RFD extracts the turn loop and its supporting components from `jp_cli` into
a new `jp_agent` crate. The crate provides a self-contained agent execution
engine with trait-based hooks for I/O. This separates the core agent logic
from CLI-specific concerns like terminal rendering, interactive prompts, and
process lifecycle.

## Motivation

The turn loop (`run_turn_loop`) and its supporting components —
`TurnCoordinator`, `ToolCoordinator`, `ChatResponseRenderer`,
`InterruptHandler`, `StreamRetryState`, and the inquiry backend — currently live
inside `jp_cli::cmd::query`. They are `pub(crate)` and tightly coupled to the
CLI binary.

This creates two problems:

**The agent loop cannot be called from outside `jp_cli`.** Any consumer that
wants to run the turn loop with different I/O backends (different output
sinks, non-terminal prompt backends, programmatic signal injection) cannot
do so. The code is locked inside the CLI crate with `pub(crate)` visibility.

**The boundary between "run the agent" and "interact with the user" is
implicit.** The turn loop directly uses `TerminalPromptBackend`, references
`crate::error::Error`, and calls `crate::signals::SignalTo`. These are CLI
concerns baked into the agent logic. The `PromptBackend` trait exists as a seam
for testing, but the turn loop still assumes a specific signal type and error
type.

Extracting the agent loop forces a clean API surface: here is a function that
runs a conversation turn given a provider, tools, config, and a set of I/O
hooks. The caller decides what those hooks do — write to a terminal, discard,
or something else entirely.

## Design

### What Moves

The following modules move from `jp_cli::cmd::query` into `jp_agent`:

| Current location       | New location                   | What it is                 |
|------------------------|--------------------------------|----------------------------|
| `turn_loop.rs`         | `jp_agent::turn_loop`          | The main agent loop        |
| `turn/coordinator.rs`  | `jp_agent::turn::coordinator`  | Turn state machine         |
| `turn/state.rs`        | `jp_agent::turn::state`        | Per-turn accumulated state |
| `stream/retry.rs`      | `jp_agent::stream::retry`      | Stream retry logic         |
| `tool/coordinator.rs`  | `jp_agent::tool::coordinator`  | Parallel tool execution    |
| `tool/inquiry.rs`      | `jp_agent::tool::inquiry`      | LLM inquiry backend        |
| `interrupt/handler.rs` | `jp_agent::interrupt::handler` | Interrupt menu logic       |
| `interrupt/signals.rs` | `jp_agent::interrupt::signals` | Signal dispatch            |
### What Stays in `jp_cli`

| Module                          | Why it stays                             |
|---------------------------------|------------------------------------------|
| `query.rs`                      | Arg parsing, config resolution, editor,  |
|                                 | conversation selection                   |
| `stream/renderer.rs`            | `ChatResponseRenderer` — implements      |
|                                 | `ResponseRenderer` using `jp_md`.        |
|                                 | Presentation concern.                    |
| `stream/structured_renderer.rs` | `StructuredRenderer` — same pattern.     |
| `tool/executor.rs`              | `TerminalExecutorSource` — creates real  |
|                                 | executors from resolved definitions.     |
|                                 | CLI-specific because it references       |
|                                 | workspace tool resolution.               |
| `tool/prompter.rs`              | `ToolPrompter` — renders                 |
|                                 | permission/question prompts to the       |
|                                 | terminal. The trait boundary is          |
|                                 | `PromptBackend`; the implementation      |
|                                 | stays in CLI.                            |
| `tool/renderer.rs`              | `ToolRenderer` — renders tool call       |
|                                 | progress/headers. Display-only, tied to  |
|                                 | terminal UX.                             |
| `tool/builtins.rs`              | Builtin tool registration (e.g.          |
|                                 | `describe_tools`).                       |
### The Boundary

The key insight: the turn loop's external dependencies are already mostly
trait-based.

| Dependency      | Current type                      | Abstraction                            |
|-----------------|-----------------------------------|----------------------------------------|
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

The concrete dependencies that need abstraction or relocation:

**`SignalTo`** — currently defined in `jp_cli::signals`. This is a simple enum
(`Shutdown`, `Quit`, `ReloadFromDisk`). It moves to `jp_agent` since the agent
loop needs to react to signals regardless of where they originate.

**`crate::error::Error`** — the turn loop returns `crate::error::Error`, which
is CLI-specific. `jp_agent` defines its own error type that the CLI wraps.

**`Workspace`** — the turn loop calls `workspace.get_events()`,
`workspace.get_events_mut()`, `workspace.persist_active_conversation()`. These
are the persistence operations. Rather than depending on `jp_workspace`
directly, `jp_agent` uses a `ConversationStore` trait defined in
`jp_conversation`:

```rust
pub trait ConversationStore {
    fn get_events(&self, id: &ConversationId) -> Option<&ConversationStream>;
    fn get_events_mut(&mut self, id: &ConversationId) -> Option<&mut ConversationStream>;
    fn persist(&mut self) -> Result<()>;
}
```

`jp_workspace::Workspace` implements the trait. Other callers can provide
their own implementation.

**`ToolRenderer`** and **`ToolPrompter`** — these are passed into the turn loop
as parameters. They stay concrete (defined in `jp_cli`) and passed in by the
caller. The turn loop accepts them via traits or generic parameters:

`ToolRenderer` is used for display (progress indicators, tool call headers).
Rather than making `ToolRenderer`
a trait, the turn loop accepts an `Arc<Printer>` (which it already does) and the
renderer is constructed inside the loop from the printer. The printer's writers
determine where output goes.

`ToolPrompter` wraps `PromptBackend` and adds CLI-specific formatting
(permission prompts with colored output, editor integration for result editing).
The turn loop already accepts `Arc<dyn PromptBackend>` — the `ToolPrompter` is
constructed inside the loop from this backend. The CLI passes
`TerminalPromptBackend`; other callers provide their own implementation.

### `run_turn_loop` Signature

The extracted function signature:

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

This is close to the current signature. The main changes:
- `&mut Workspace` → `&mut dyn ConversationStore`
- `crate::error::Error` → `AgentError`
- `SignalRx` and `SignalTo` come from `jp_agent`

### `ToolRenderer` Stays Internal

`ToolRenderer` is constructed inside `run_turn_loop` today:

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

This construction stays inside the extracted `run_turn_loop`. `ToolRenderer`
moves to `jp_agent` alongside the turn loop. Its output goes through the
`Printer`, which the caller controls via writer injection.

### Response Rendering

The `TurnCoordinator` currently owns a `ChatResponseRenderer` (which depends
on `jp_md`) and calls it on every streamed `ChatResponse` chunk. The agent
loop controls *when* rendering happens (on each chunk, flush before tool calls,
reset on continuation), but *how* rendering works is a presentation concern.

`jp_agent` defines a rendering trait:

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

Three methods, each with clear semantics for any implementation:

- A terminal renderer (`ChatResponseRenderer`) buffers markdown, formats with
  ANSI styling, and writes to the printer.
- A no-op renderer discards everything (e.g. non-interactive execution).
- A test renderer captures events for assertions.

The current `reset_content_kind()` call in `TurnCoordinator` — which exists
because the markdown renderer tracks reasoning vs. message transitions — becomes
an internal concern of the `ChatResponseRenderer` implementation. The agent loop
just calls `flush()` when a tool call arrives; the renderer decides what
bookkeeping that entails.

`StructuredRenderer` follows the same pattern, either folded into
`ResponseRenderer` or as a parallel trait.

This removes `jp_md` from `jp_agent`'s dependency graph entirely.

### Crate Dependencies

`jp_agent` depends on:
- `jp_config` (for `AppConfig`, `StyleConfig`, tool config types)
- `jp_conversation` (for `ConversationStream`, events, `ConversationId`, `ConversationStore`)
- `jp_llm` (for `Provider`, `Event`, `ToolDefinition`, `Executor`)
- `jp_printer` (for `Printer`)
- `jp_inquire` (for `PromptBackend`)
- `jp_mcp` (for `Client`)
- `jp_attachment` (for `Attachment`)
- `jp_tool` (for `Question`, `AnswerType`)

`jp_cli` depends on `jp_agent` and provides:
- `TerminalPromptBackend` (via `jp_inquire`)
- `TerminalExecutorSource` (creates real executors)
- `Workspace` as `ConversationStore`
- OS signal setup
- Arg parsing, editor, config resolution

### What `jp_cli::cmd::query` Becomes

After extraction, `query.rs` is the orchestration layer:

1. Parse args, resolve config, select conversation
2. Open editor, build `ChatRequest`
3. Resolve tools, set up MCP, register attachments
4. Create a `ToolCoordinator` with `TerminalExecutorSource`
5. Create the `LlmInquiryBackend`
6. Call `jp_agent::run_turn_loop(...)` with terminal-backed I/O
7. Handle the result, persist, clean up

This is essentially what `Query::run()` does today, minus the turn loop itself.

## Drawbacks

**More crates.** The workspace gains another crate. Build times increase
slightly. The dependency graph gets one more node.

**Parameter proliferation.** `run_turn_loop` already has 16 parameters. Adding a
trait for persistence doesn't reduce this. A builder or context struct could
help, but that's a separate refactor.

**Incremental move.** Moving modules between crates requires updating every
import path. Tests that reference internal types need adjustment. This is
mechanical but tedious.

## Alternatives

### Move only the turn loop, keep renderers in CLI

Move `run_turn_loop` and the coordinators, but leave
`ChatResponseRenderer`, `ToolRenderer`, and the interrupt handler in `jp_cli`.
Pass them to the turn loop as trait objects or closures.

Rejected because the renderers depend on `jp_md` for markdown formatting,
which is a presentation concern. Moving them into `jp_agent` would give the
agent crate a dependency on markdown rendering. The `ResponseRenderer` trait
provides a clean boundary: the agent loop calls `render`/`flush`/`reset`,
and the caller provides the implementation.

### Use `jp_llm` instead of a new crate

Put the agent loop in `jp_llm` since it already hosts the provider traits
and tool execution types.

Rejected because `jp_llm` is a provider abstraction layer. The agent loop
depends on `jp_printer`, `jp_config` and `jp_conversation` - concerns that don't
belong in the LLM layer. A separate crate keeps the dependency graph clean.

### Trait for everything (full DI)

Define traits for `Printer`, `ToolRenderer`, signal handling, and every other
concrete dependency. Make the turn loop fully generic.

Rejected as over-engineering. The `Printer` already supports writer injection.
`ToolRenderer` is an internal detail of the loop, not an extension point.
Signals are a simple enum. Full DI adds type parameters and complexity without
enabling use cases beyond what `ConversationStore` + `PromptBackend` provide.

## Non-Goals

- **Public SDK.** `jp_agent` is an internal crate for use within the JP
  workspace. Its API is not stable and not documented for external consumers. A
  public SDK can be built on top of it later, but that's a separate effort with
  different design constraints.

- **Plugin system.** The trait-based hooks enable different I/O backends, not
  arbitrary agent behavior customization. Hooks for tool filtering, response
  post-processing, or custom turn logic are out of scope.

- **Async trait simplification.** Some traits (`InquiryBackend`,
  `ExecutorSource`) use `#[async_trait]`. This RFD does not change their
  signatures or migrate to native async traits.

## Risks and Open Questions

### `ToolPrompter` and editor integration

`ToolPrompter` currently handles permission prompts with editor integration (the
user can edit tool arguments before approving). The editor is a CLI concern —
it opens `$EDITOR` and reads the result. Other callers would need a different
mechanism for argument editing.

For the initial extraction, `ToolPrompter` is constructed in the turn loop from
the injected `PromptBackend`. The `PromptBackend` trait may need extension to
support argument editing, or `ToolPrompter` needs its own trait. This can be
resolved during implementation.

### MCP client dependency

The turn loop takes `&jp_mcp::Client` for tool execution (some tools are
MCP-backed). This is a direct dependency that flows through to `ToolCoordinator`
and `Executor`. For now, `jp_mcp::Client` stays as a parameter. Whether this
should be abstracted behind a trait is left for future work.

### `is_tty` semantics

The turn loop uses `is_tty` to control progress indicators and `ToolRenderer`
behavior. This is a property of the output consumer, not the process. For
the initial extraction, `is_tty` stays as a `bool` parameter. If a future
caller needs this to change at runtime (e.g. output consumer capabilities
change mid-turn), the parameter would need to become a dynamic check.

## Implementation Plan

### Phase 1: Create `jp_agent` Crate, Move Types

Create the `jp_agent` crate. Move `SignalTo` (and the signal-related types)
and `TurnState` first — these have no internal dependencies on CLI types.
Define `AgentError`.

`jp_cli` re-exports or wraps as needed. All existing tests pass.

Add `ConversationStore` trait to `jp_conversation`.

Can be merged independently.

### Phase 2: Move Turn Coordinator and Stream Retry

Move `TurnCoordinator` and `StreamRetryState` into `jp_agent`. Add the
`ResponseRenderer` trait. `TurnCoordinator` takes `Box<dyn ResponseRenderer>`
instead of owning `ChatResponseRenderer` directly.

`ChatResponseRenderer` and `StructuredRenderer` stay in `jp_cli` and
implement the new trait.

Update `jp_cli` imports.

Can be merged independently.

### Phase 3: Move Tool Coordinator and Interrupt Handler

Move `ToolCoordinator` and `InterruptHandler` into `jp_agent`. These are the
components that interact with `PromptBackend` and signals — the I/O boundary.

Replace `crate::Error` references in the moved code with `AgentError`.

Can be merged independently.

### Phase 4: Move Turn Loop

Move `run_turn_loop` into `jp_agent`. Replace `&mut Workspace` with
`&mut dyn ConversationStore`. Update `jp_cli::cmd::query` to call
`jp_agent::run_turn_loop(...)` with the workspace as the store and
terminal backends for I/O.

This is the final step. After this, `jp_cli::cmd::query::run()` is purely
orchestration — config resolution, conversation selection, and calling the
extracted agent loop.

Depends on Phases 1–3.

## References

- [`run_turn_loop`](../../crates/jp_cli/src/cmd/query/turn_loop.rs) — the
  current turn loop implementation being extracted.
- [`PromptBackend`](../../crates/jp_inquire/src/prompt.rs) — the existing
  trait that enables I/O injection for prompts.
- [`ExecutorSource`](../../crates/jp_llm/src/tool/executor.rs) — the existing
  trait for tool executor creation.
- [yoagent](https://github.com/yologdev/yoagent) — prior art for a standalone
  Rust agent loop library.
