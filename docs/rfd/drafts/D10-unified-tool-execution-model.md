# RFD D10: Unified Tool Execution Model

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-01

## Summary

This RFD introduces a `ToolRuntime` trait that abstracts how tools execute, and
a `runtime` configuration field on `ToolConfig` that selects the execution
environment for local tools. The trait replaces the current three-way dispatch
in `ToolDefinition::execute()` with a single polymorphic call. Four initial
implementations cover every existing tool source: `StdioRuntime` (subprocess),
`McpRuntime` (MCP server RPC), `BuiltinRuntime` (in-process Rust), and — in
future — a `WasmRuntime` as specified by [RFD 016]. Tool resolution produces a
`(ToolDefinition, Arc<dyn ToolRuntime>)` pair; the coordinator calls
`runtime.execute()` without knowing the underlying mechanism. `ToolSource` is
unchanged — it describes where a tool's definition comes from, not how it runs.

## Motivation

Tool execution in JP is currently dispatched through a `match` on `ToolSource`
inside `ToolDefinition::execute()`:

```rust
match config.source() {
    ToolSource::Local { .. }   => self.execute_local(...).await,
    ToolSource::Mcp { .. }    => self.execute_mcp(...).await,
    ToolSource::Builtin { .. } => self.execute_builtin(...).await,
}
```

Each arm has its own argument handling, error mapping, and cancellation logic.
The `execute_local` path builds a JSON context, renders templates, spawns a
subprocess, and parses the output. `execute_mcp` calls the MCP client.
`execute_builtin` looks up a hardcoded executor. These are three independent
code paths that share no interface.

This causes several problems:

1. **Every consumer of tool execution must pass all possible dependencies.** The
   `Executor::execute()` trait method takes `mcp_client: &Client` and `root:
   &Utf8Path` even though MCP tools don't use `root` and builtin tools use
   neither. The `ToolCoordinator` threads both through every call site. The
   `run_turn_loop` function takes both as parameters and passes them through
   multiple layers. Adding a new execution mechanism (WASM, VFS-mediated IPC)
   means adding more parameters to every function in the chain.

2. **Testing requires all dependencies regardless of what's under test.** The
   `MockExecutor` ignores `mcp_client` and `root`, but they must still be
   provided. Integration tests in `turn_loop_tests.rs` create temporary
   directories for `root` even when testing MCP or builtin tool behavior. A
   test for builtin tool coordination shouldn't need a filesystem path.

3. **New execution mechanisms require modifying the dispatch.** Adding [RFD 016]
   WASM support means adding a fourth arm to the match and a fourth set of
   parameters threaded through the call chain. A future VFS-mediated IPC
   runtime would be a fifth. Each addition touches `ToolDefinition`,
   `Executor`, `ToolCoordinator`, `run_turn_loop`, and `handle_turn`.

4. **Source and execution are conflated.** `ToolSource::Local` implies subprocess
   execution. But a local tool could run in a WASM sandbox or through a
   VFS-mediated IPC channel — "where the definition comes from" and "how it
   runs" are independent axes. The current design cannot express a local tool
   with WASM execution without introducing a new `ToolSource` variant that
   mixes source identity with execution mechanics.

The fix is to extract execution into a trait. Tool resolution produces a runtime
object alongside the definition. The coordinator calls the runtime without
knowing what's behind it. Dependencies are captured at construction time, not
threaded through call sites.

## Design

### The `ToolRuntime` Trait

`ToolRuntime` abstracts the execution of a single tool call. It is the
runtime-side counterpart of `ToolDefinition`, which describes a tool's schema
and metadata.

```rust
/// Abstracts how a tool is executed.
///
/// Each implementation captures its own dependencies at construction
/// time. The coordinator calls `execute()` without knowing whether
/// the tool runs as a subprocess, an MCP call, a builtin function,
/// or a WASM component.
#[async_trait]
pub trait ToolRuntime: Send + Sync {
    /// Execute the tool with the given arguments and return the outcome.
    ///
    /// `answers` contains accumulated responses to previous `NeedsInput`
    /// outcomes (the inquiry/question loop). `cancellation_token` signals
    /// that the user or system has requested cancellation.
    async fn execute(
        &self,
        name: &str,
        id: String,
        arguments: Value,
        answers: &IndexMap<String, Value>,
        config: &ToolConfigWithDefaults,
        cancellation_token: CancellationToken,
    ) -> Result<ExecutionOutcome, ToolError>;
}
```

Each runtime captures its own dependencies at construction time:

- `StdioRuntime` holds the project root path (or a `ProjectFiles` reference
  once [RFD D09] is implemented).
- `McpRuntime` holds an `Arc<jp_mcp::Client>`.
- `BuiltinRuntime` holds a `BuiltinExecutors` registry.
- A future `WasmRuntime` would hold the `wasmtime::Engine` and component cache.

The coordinator and executor no longer need `mcp_client: &Client` or `root:
&Utf8Path` as parameters — those are internal to the runtime that needs them.

### Runtime Implementations

#### `StdioRuntime`

Wraps the current `execute_local` logic: build a JSON context from the tool
name, arguments, answers, and options; render templates; spawn a subprocess;
parse `jp_tool::Outcome` from stdout.

```rust
pub struct StdioRuntime {
    root: Utf8PathBuf,
}
```

The `root` field becomes `Arc<dyn ProjectFiles>` once [RFD D09] is implemented.
For now, it remains a path. The subprocess receives `root` as `current_dir` and
in the tool context JSON, exactly as today.

`StdioRuntime` is the default runtime for `source = "local"` tools.

#### `McpRuntime`

Wraps the current `execute_mcp` logic: call the MCP server, collect content
blocks, map errors.

```rust
pub struct McpRuntime {
    client: Arc<jp_mcp::Client>,
}
```

`McpRuntime` is the sole runtime for `source = "mcp.*"` tools. The MCP client
is shared across all MCP tool calls.

#### `BuiltinRuntime`

Wraps the current `execute_builtin` logic: look up the tool in the executor
registry, call it with arguments and answers.

```rust
pub struct BuiltinRuntime {
    executors: Arc<BuiltinExecutors>,
}
```

`BuiltinRuntime` is the sole runtime for `source = "builtin"` tools.

#### Future: `WasmRuntime`

When [RFD 016] is implemented, `WasmRuntime` would instantiate a WASM
component, provide `jp:host/*` imports, and call the tool's exported function.
This RFD does not define `WasmRuntime` — it only establishes the trait that a
future implementation would satisfy.

#### Future: `VfsRuntime`

A future RFD will define a VFS-mediated IPC runtime where the subprocess
accesses host resources (files, network, processes) through a JSON-RPC protocol
over stdin/stdout rather than direct system calls. This runtime would implement
the same `ToolRuntime` trait. This RFD does not define `VfsRuntime`.

### The `runtime` Configuration Field

A new optional field on `ToolConfig` selects the execution environment for
local tools:

```rust
pub enum ToolRuntimeKind {
    /// Subprocess with direct filesystem access (current behavior).
    Stdio,

    /// Subprocess with VFS-mediated IPC (future).
    Vfs,

    /// WASM component (future, per RFD 016).
    Wasm,
}
```

```toml
# Explicit runtime selection
[tools.my_vfs_tool]
source = "local"
command = ".config/jp/tools/target/release/jp-tools fs modify_file"
runtime = "vfs"

# WASM runtime inferred from .wasm extension
[tools.my_plugin]
source = "local"
command = ".jp/plugins/my_tool.wasm"

# WASM runtime explicit (non-.wasm binary)
[tools.my_other_plugin]
source = "local"
command = ".jp/plugins/my_tool"
runtime = "wasm"

# Default: runtime = "stdio" (omitted)
[tools.cargo_check]
source = "local"
command = ".config/jp/tools/target/release/jp-tools cargo check"
```

Resolution order for `runtime`:

1. If `runtime` is set explicitly, use it.
2. If `command` ends in `.wasm`, infer `runtime = "wasm"`.
3. Default to `"stdio"`.

The `runtime` field only applies to `source = "local"` tools. For `mcp` and
`builtin` sources, the runtime is determined by the source — there is only one
way to execute them. The field is ignored (with a warning) if set on a non-local
tool.

At this time, only `stdio` is implemented. Configuring `runtime = "vfs"` or
`runtime = "wasm"` (or a `.wasm` command) produces a clear error:

```text
Error: Tool 'my_tool' uses runtime 'vfs', which is not yet supported.
```

This makes the config surface forward-compatible without requiring
implementation of all runtimes upfront.

### Runtime Resolution

Tool resolution currently happens in `tool_definitions()` (in `jp_llm/src/
tool.rs`), which iterates tool configs, checks enablement, and resolves
definitions from local config or MCP servers. This function returns
`Vec<ToolDefinition>`.

After this RFD, resolution produces `Vec<(ToolDefinition, Arc<dyn
ToolRuntime>)>`. The runtime is selected based on source and config:

```rust
fn resolve_runtime(
    config: &ToolConfigWithDefaults,
    stdio: &Arc<StdioRuntime>,
    mcp: &Arc<McpRuntime>,
    builtin: &Arc<BuiltinRuntime>,
) -> Result<Arc<dyn ToolRuntime>, ToolError> {
    match config.source() {
        ToolSource::Builtin { .. } => Ok(Arc::clone(builtin) as _),
        ToolSource::Mcp { .. } => Ok(Arc::clone(mcp) as _),
        ToolSource::Local { .. } => {
            match config.runtime_kind() {
                ToolRuntimeKind::Stdio => Ok(Arc::clone(stdio) as _),
                ToolRuntimeKind::Vfs => Err(ToolError::UnsupportedRuntime("vfs")),
                ToolRuntimeKind::Wasm => Err(ToolError::UnsupportedRuntime("wasm")),
            }
        }
    }
}
```

Runtime instances are created once at the start of `jp query` and shared across
all tool calls of the same type. `StdioRuntime`, `McpRuntime`, and
`BuiltinRuntime` are all `Send + Sync` and stateless (or hold `Arc`-wrapped
shared state), so sharing is safe.

### Changes to `Executor` and `ToolCoordinator`

The `Executor` trait (in `jp_llm/src/tool/executor.rs`) currently takes
`mcp_client` and `root` as parameters:

```rust
async fn execute(
    &self,
    answers: &IndexMap<String, Value>,
    mcp_client: &Client,
    root: &Utf8Path,
    cancellation_token: CancellationToken,
) -> ExecutorResult;
```

After this RFD, those parameters are removed. The executor holds an `Arc<dyn
ToolRuntime>` and delegates to it:

```rust
async fn execute(
    &self,
    answers: &IndexMap<String, Value>,
    cancellation_token: CancellationToken,
) -> ExecutorResult;
```

The `ToolExecutor` (in `jp_cli`) is updated to hold the runtime:

```rust
pub struct ToolExecutor {
    request: ToolCallRequest,
    config: ToolConfigWithDefaults,
    definition: ToolDefinition,
    runtime: Arc<dyn ToolRuntime>,
}
```

Its `execute` method delegates to the runtime:

```rust
async fn execute(
    &self,
    answers: &IndexMap<String, Value>,
    cancellation_token: CancellationToken,
) -> ExecutorResult {
    let result = self.runtime.execute(
        &self.request.name,
        self.request.id.clone(),
        Value::Object(self.request.arguments.clone()),
        answers,
        &self.config,
        cancellation_token,
    ).await;

    match result {
        Ok(ExecutionOutcome::Completed { id, result }) => {
            ExecutorResult::Completed(ToolCallResponse { id, result })
        }
        Ok(ExecutionOutcome::NeedsInput { id: _, question }) => {
            ExecutorResult::NeedsInput { /* ... */ }
        }
        Ok(ExecutionOutcome::Cancelled { id }) => {
            ExecutorResult::Completed(ToolCallResponse {
                id,
                result: Ok("Tool execution cancelled.".to_string()),
            })
        }
        Err(e) => {
            ExecutorResult::Completed(ToolCallResponse {
                id: self.request.id.clone(),
                result: Err(e.to_string()),
            })
        }
    }
}
```

The `TerminalExecutorSource` is updated to receive runtimes at construction and
pass them to each `ToolExecutor`:

```rust
pub struct TerminalExecutorSource {
    definitions: IndexMap<String, (ToolDefinition, Arc<dyn ToolRuntime>)>,
}
```

`ToolCoordinator::execute_with_prompting` and `spawn_tool_execution` drop
their `mcp_client` and `root` parameters. The coordinator no longer knows or
cares what dependencies a runtime needs.

### Changes to `run_turn_loop`

The `run_turn_loop` function currently takes `mcp_client` and `root` as
parameters and threads them through to the coordinator. After this RFD, those
parameters are removed from `run_turn_loop`'s signature because runtimes are
already captured inside the executor source.

`mcp_client` is still needed by the *inquiry backend* (for LLM-answered tool
questions), so it remains as a parameter for that purpose. But it no longer
flows through the tool execution path.

The `root` parameter is removed entirely from `run_turn_loop`. It currently
also serves the `ToolRenderer` (for custom argument formatting), which will
receive it through the `StdioRuntime` or, more practically, continue to receive
it directly since rendering is a CLI concern, not a runtime concern.

### What Stays the Same

- **`ToolSource`** is unchanged. It remains a string-based enum (`builtin`,
  `local`, `mcp.<server>.<tool>`) that describes where a tool's definition
  comes from.
- **`ToolDefinition`** is unchanged. It describes a tool's name, docs, and
  parameters.
- **`ExecutionOutcome`** is unchanged. It is the return type of both the
  current dispatch and the new `ToolRuntime::execute()`.
- **`ToolConfigWithDefaults`** is unchanged (except for the new `runtime`
  field). It carries all per-tool configuration.
- **Tool behavior** is unchanged. Every existing tool produces identical results
  before and after this refactor.

### Crate Boundaries

`ToolRuntime` and `ToolRuntimeKind` live in `jp_llm`, alongside the existing
`ToolDefinition` and `ExecutionOutcome` types. This keeps all tool execution
abstractions in one crate.

The concrete runtime implementations live where their dependencies are:

- `StdioRuntime` — in `jp_llm`, since it uses `run_tool_command` which is
  already there.
- `McpRuntime` — in `jp_llm`, since it already contains `execute_mcp`.
- `BuiltinRuntime` — in `jp_llm`, since it already contains `execute_builtin`.

The `jp_cli` crate constructs the runtime instances (it knows the workspace
root, the MCP client, and the builtin registry) and passes them into the
resolution layer.

## Drawbacks

- **Indirection.** The current `match` dispatch is explicit and easy to follow
  in a debugger. A trait object adds a vtable lookup and makes it harder to
  "go to definition" from a call site. For three implementations, the `match`
  is arguably simpler.

- **Runtime construction is front-loaded.** All runtime instances are created at
  the start of `jp query`, even if no tools of a given type are used. This is
  cheap (the runtimes are small structs wrapping `Arc`s) but represents
  allocated-but-unused objects for some invocations.

- **Config field for future use.** The `runtime` field accepts `"vfs"` and
  `"wasm"` values that produce errors today. This is forward-compatible
  but could confuse users who try to use them before the implementations exist.
  The error message must be clear about this.

## Alternatives

### Keep the `match` dispatch

Leave the three-way match in `ToolDefinition::execute()`. Add arms as new
execution mechanisms arrive.

Rejected because it requires threading every runtime's dependencies through the
entire call chain. The parameter lists on `Executor::execute()`,
`ToolCoordinator::execute_with_prompting()`, `spawn_tool_execution()`, and
`run_turn_loop()` already have too many parameters. Adding WASM and VFS
dependencies would make this worse.

### Put `runtime` into `ToolSource`

Replace `ToolSource::Local` with `ToolSource::Stdio`, `ToolSource::Vfs`,
`ToolSource::Wasm`. The source enum directly determines the execution
mechanism.

Rejected because it conflates two orthogonal concerns. `ToolSource` answers
"where does the definition come from?" (local config, MCP server, hardcoded
builtin). `runtime` answers "how does it execute?" (subprocess, IPC, WASM).
A local tool can execute as a subprocess today and as a WASM component tomorrow
without changing its source. Mixing them in one enum prevents this and forces
the enum to grow with the product of sources × runtimes.

### Use an enum instead of a trait

```rust
enum ToolRuntime {
    Stdio(StdioRuntime),
    Mcp(McpRuntime),
    Builtin(BuiltinRuntime),
}
```

Dispatch via `match` on the enum variants instead of dynamic dispatch.

Rejected because it closes the extension point. The `ToolRuntime` trait is
designed to support future runtimes (WASM, VFS) without modifying the enum.
Plugin-provided runtimes (a possibility once [RFD 016] matures) would also
need the open extension point. The vtable cost is negligible at I/O boundaries.

## Non-Goals

- **Implementing `VfsRuntime` or `WasmRuntime`.** This RFD defines the trait
  and config surface they will use. The implementations are scoped to future
  RFDs.

- **Changing tool behavior.** Every tool produces identical results before and
  after this refactor. This is a structural change, not a behavioral one.

- **Sandboxing.** Tool sandboxing is addressed by [RFD 075]. This RFD does not
  add security restrictions to tool execution.

- **Stateful tool protocol.** [RFD 009] defines multi-turn tool interaction
  (spawn/fetch/apply/abort). The `ToolRuntime` trait covers single-execution
  invocations. When [RFD 009] is implemented, the stateful lifecycle will be
  managed above the runtime layer — the runtime executes one step, and the
  coordinator manages state transitions.

- **Self-describing tools.** [RFD D06] and [RFD D07] improve how tool schemas
  are authored and discovered. Those are orthogonal to how tools execute.

## Risks and Open Questions

### `ToolRenderer` and `root`

The `ToolRenderer` in `jp_cli` uses `root` for custom argument formatting (it
runs a subprocess to format arguments). After this RFD, `root` is no longer
threaded through `run_turn_loop`. The renderer will need to receive `root`
through its own construction, not through the execution path. This is
straightforward but is a change to the renderer's initialization.

### Inquiry backend and `mcp_client`

The inquiry backend (for LLM-answered tool questions) needs the MCP client to
resolve tool definitions for the inquiry conversation. This dependency remains
on `run_turn_loop` even after tool execution no longer needs it. This is
correct — the inquiry backend is not a tool runtime concern — but it means
`mcp_client` doesn't fully disappear from the turn loop signature.

### `config` parameter on `ToolRuntime::execute()`

The trait method receives `&ToolConfigWithDefaults`, which carries the full
tool configuration including `command`, `source`, `parameters`, `options`, etc.
The runtime uses what it needs (`command` for stdio, nothing for builtin) and
ignores the rest. An alternative is to pass only runtime-specific config (e.g.,
`command` for stdio), but this would require per-runtime config extraction
logic that adds complexity without clear benefit. The current approach is
pragmatic: pass the full config, let each runtime take what it needs.

### Migration of `execute_local` internals

`execute_local` currently handles parameter defaults, argument validation,
context JSON construction, and command execution. All of this moves into
`StdioRuntime::execute()`. The logic is unchanged; only its location changes.
The risk is introducing bugs during the move. The existing test suite
(`tool_tests.rs`, `turn_loop_tests.rs`, `coordinator_tests.rs`) provides
coverage, and all tests must pass unchanged after the refactor.

## Implementation Plan

### Phase 1: Define `ToolRuntime` trait and `ToolRuntimeKind` config

Add the `ToolRuntime` trait to `jp_llm`. Add the `ToolRuntimeKind` enum and
`runtime` field to `ToolConfig` in `jp_config`. Implement config parsing,
serialization, and the `.wasm` inference logic. No runtime implementations yet.

**Depends on:** Nothing.
**Mergeable:** Yes.

### Phase 2: Implement `StdioRuntime`

Extract `execute_local` from `ToolDefinition` into `StdioRuntime`. The
`StdioRuntime::execute()` method contains the same logic: parameter defaults,
argument validation, context building, subprocess spawning, output parsing.

**Depends on:** Phase 1.
**Mergeable:** Yes.

### Phase 3: Implement `McpRuntime` and `BuiltinRuntime`

Extract `execute_mcp` into `McpRuntime` and `execute_builtin` into
`BuiltinRuntime`.

**Depends on:** Phase 1.
**Mergeable:** Yes (parallel with Phase 2).

### Phase 4: Update `Executor` trait and `ToolCoordinator`

Remove `mcp_client` and `root` from `Executor::execute()`. Update
`ToolExecutor` to hold `Arc<dyn ToolRuntime>`. Update `TerminalExecutorSource`
to receive runtime-paired definitions. Update `ToolCoordinator` and
`spawn_tool_execution` to drop the removed parameters.

**Depends on:** Phases 2 and 3.
**Mergeable:** Yes.

### Phase 5: Update `run_turn_loop` and `handle_turn`

Remove `root` from `run_turn_loop`'s parameter list. Update `ToolRenderer` to
receive `root` at construction. Update `handle_turn` in `jp_cli` to construct
runtimes and pass them into the resolution layer. Remove `root` threading from
the call chain.

**Depends on:** Phase 4.
**Mergeable:** Yes.

### Phase 6: Update `MockExecutor` and `TestExecutorSource`

Remove `mcp_client` and `root` from `MockExecutor::execute()` and
`TestExecutorSource`. Update all test helpers and assertions. All existing tests
must pass unchanged.

**Depends on:** Phase 4.
**Mergeable:** Yes (parallel with Phase 5).

### Phase 7: Remove `ToolDefinition::execute()` dispatch

Once all call sites use `ToolRuntime`, remove the `execute()`, `execute_local`,
`execute_mcp`, and `execute_builtin` methods from `ToolDefinition`. The
dispatch logic is fully replaced by runtime resolution.

**Depends on:** Phases 5 and 6.
**Mergeable:** Yes.

## References

- [RFD 009] — Stateful tool protocol. Defines multi-turn tool interaction;
  `ToolRuntime` covers single-execution steps within that model.
- [RFD 016] — WASM plugin architecture. A future `WasmRuntime` would implement
  `ToolRuntime` using the WASM plugin infrastructure.
- [RFD 075] — Tool sandbox and access policy. Sandboxing is orthogonal to the
  runtime model; sandbox profiles are applied by the runtime implementation
  (e.g., `StdioRuntime` applies `sandbox-exec` before spawning).
- [RFD D06] — Self-describing local tools. Schema discovery is orthogonal to
  execution; it happens at resolution time, before the runtime is invoked.
- [RFD D07] — Typed tool SDK for Rust. SDK improvements affect tool authoring,
  not execution dispatch.
- [RFD D09] — Project filesystem abstraction. `StdioRuntime`'s `root` field
  will evolve into `Arc<dyn ProjectFiles>` when this RFD is implemented.

[RFD 009]: 009-stateful-tool-protocol.md
[RFD 016]: 016-wasm-plugin-architecture.md
[RFD 075]: 075-tool-sandbox-and-access-policy.md
[RFD D06]: D06-self-describing-local-tools.md
[RFD D07]: D07-typed-tool-sdk-for-rust.md
[RFD D09]: D09-project-filesystem-abstraction.md
