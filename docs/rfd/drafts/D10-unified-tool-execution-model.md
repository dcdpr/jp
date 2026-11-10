# RFD D10: Unified Tool Execution Model

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-01
- **Required by**: [RFD D11]
- **Summary**: Introduces a `ToolRuntime` trait and a `runtime` tool setting to
  unify local, MCP, built-in, and Wasm execution.

## Summary

This RFD introduces a `ToolRuntime` trait for executing tools and a `runtime`
field on `ToolConfig` that selects how a local tool runs.
Tool resolution pairs each `ToolDefinition` with an `Arc<dyn ToolRuntime>`
(`StdioRuntime`, `McpRuntime`, `BuiltinRuntime`, or a future `WasmRuntime` from
[RFD 016]) so the coordinator can call `runtime.execute()` polymorphically.
`ToolSource` remains responsible only for where a tool's definition originates.

## Motivation

`ToolDefinition::execute()` dispatches tool calls by matching on `ToolSource`:

```rust
match config.source() {
    ToolSource::Local { .. }   => self.execute_local(...).await,
    ToolSource::Mcp { .. }    => self.execute_mcp(...).await,
    ToolSource::Builtin { .. } => self.execute_builtin(...).await,
}
```

Each arm is an independent code path with its own argument handling, error
mapping, and cancellation logic: `execute_local` builds a JSON context, renders
templates, spawns a subprocess, and parses stdout; `execute_mcp` calls the MCP
client; and `execute_builtin` looks up a hardcoded executor.

This structure causes four problems:

1. **Callers must thread every runtime's dependencies.** `Executor::execute()`
   takes both `mcp_client: &Client` and `root: &Utf8Path`, even though MCP tools
   ignore `root` and builtin tools use neither.
   `run_turn_loop` and `ToolCoordinator` pass both parameters through every
   layer of the call chain.

2. **Tests must construct unused dependencies.** `MockExecutor` ignores
   `mcp_client` and `root`, yet callers must still provide them.
   Integration tests in `turn_loop_tests.rs` create temporary directories for
   `root` even when testing only MCP or builtin tool coordination.

3. **Adding a runtime touches the entire call chain.** Supporting WASM ([RFD
   016]) or a VFS-mediated IPC runtime requires a new `match` arm and another
   set of parameters threaded through `handle_turn`, `run_turn_loop`,
   `ToolCoordinator`, `Executor`, and `ToolDefinition`.

4. **Source and execution are conflated.** `ToolSource::Local` implies
   subprocess execution, but where a tool's definition comes from and how the
   tool runs are independent axes.
   A local tool cannot run in a WASM sandbox or over a VFS-mediated IPC channel
   without adding a `ToolSource` variant that mixes source identity with
   execution mechanics.

Extracting execution into a `ToolRuntime` trait resolves these problems.
Tool resolution constructs the appropriate runtime with its dependencies
captured up front and pairs it with the `ToolDefinition`, so the coordinator
executes every tool through a single interface.

## Design

### The `ToolRuntime` Trait

`ToolRuntime` abstracts the execution of a single tool call.
It is the runtime counterpart to `ToolDefinition`, which describes a tool's
schema and metadata.

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

Each implementation captures its own dependencies at construction time, so the
coordinator and executor no longer take `mcp_client: &Client` or `root:
&Utf8Path` as parameters.

### Runtime Implementations

#### `StdioRuntime`

`StdioRuntime` is the default runtime for `source = "local"` tools.
It encapsulates the `execute_local` flow: building a JSON context from the tool
name, arguments, answers, and options; rendering templates; spawning a
subprocess with `root` passed as both `current_dir` and the tool context root;
and parsing `jp_tool::Outcome` from stdout.

```rust
pub struct StdioRuntime {
    root: Utf8PathBuf,
}
```

Once [RFD D09] is implemented, `root` will change from `Utf8PathBuf` to `Arc<dyn
ProjectFiles>`.

#### `McpRuntime`

`McpRuntime` is the sole runtime for `source = "mcp.*"` tools.
It wraps the `execute_mcp` logic, calling the shared MCP client, collecting
content blocks, and mapping server errors.

```rust
pub struct McpRuntime {
    client: Arc<jp_mcp::Client>,
}
```

#### `BuiltinRuntime`

`BuiltinRuntime` is the sole runtime for `source = "builtin"` tools.
It wraps the `execute_builtin` logic, looking up the tool in the
`BuiltinExecutors` registry and invoking it with the call arguments and inquiry
answers.

```rust
pub struct BuiltinRuntime {
    executors: Arc<BuiltinExecutors>,
}
```

#### Future: `WasmRuntime`

When [RFD 016] is implemented, `WasmRuntime` will hold the `wasmtime::Engine`
and component cache, instantiate a WASM component, provide `jp:host/*` imports,
and call the tool's exported function.
This RFD establishes the `ToolRuntime` trait that `WasmRuntime` will implement,
not `WasmRuntime` itself.

#### Future: `VfsRuntime`

A future RFD will define `VfsRuntime`, a VFS-mediated IPC runtime in which the
subprocess accesses host resources (files, network, processes) over stdin/stdout
via JSON-RPC rather than direct system calls.
Like `WasmRuntime`, `VfsRuntime` will implement `ToolRuntime` and is out of
scope for this RFD.

### The `runtime` Configuration Field

An optional `runtime` field on `ToolConfig` selects the execution environment
for local tools:

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

JP resolves `runtime` in the following order:

1. Use `runtime` if set explicitly.
2. Infer `runtime = "wasm"` if `command` ends in `.wasm`.
3. Default to `"stdio"`.

The `runtime` field applies only to `source = "local"` tools.
`mcp` and `builtin` tools each have a single runtime determined by their source;
setting `runtime` on a non-local tool logs a warning and has no effect.

Only `stdio` is implemented in this RFD.
Configuring `runtime = "vfs"`, `runtime = "wasm"`, or a `.wasm` command returns
an error:

```text
Error: Tool 'my_tool' uses runtime 'vfs', which is not yet supported.
```

Accepting these values in `ToolRuntimeKind` keeps the configuration surface
forward-compatible before `VfsRuntime` and `WasmRuntime` are implemented.

### Runtime Resolution

`tool_definitions()` in `jp_llm/src/tool.rs` iterates tool configs, checks
whether each tool is enabled, and resolves definitions from local config or MCP
servers.
Instead of `Vec<ToolDefinition>`, it returns `Vec<(ToolDefinition, Arc<dyn
ToolRuntime>)>`, selecting each tool's runtime from its source and config:

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

`jp query` constructs the runtime instances once at startup and shares them
across all tool calls of the same kind.
`StdioRuntime`, `McpRuntime`, and `BuiltinRuntime` are `Send + Sync` and hold
only immutable or `Arc`-wrapped shared state.

### Changes to `Executor` and `ToolCoordinator`

`Executor::execute()` in `jp_llm/src/tool/executor.rs` currently takes
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

With dependencies stored on the runtime, `Executor::execute()` drops both
parameters:

```rust
async fn execute(
    &self,
    answers: &IndexMap<String, Value>,
    cancellation_token: CancellationToken,
) -> ExecutorResult;
```

`ToolExecutor` in `jp_cli` holds the tool's `Arc<dyn ToolRuntime>`:

```rust
pub struct ToolExecutor {
    request: ToolCallRequest,
    config: ToolConfigWithDefaults,
    definition: ToolDefinition,
    runtime: Arc<dyn ToolRuntime>,
}
```

Its `execute` method delegates to `self.runtime` and maps the `ExecutionOutcome`
into an `ExecutorResult`:

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

`TerminalExecutorSource` receives the runtime-paired definitions at construction
and passes the matching runtime to each `ToolExecutor`:

```rust
pub struct TerminalExecutorSource {
    definitions: IndexMap<String, (ToolDefinition, Arc<dyn ToolRuntime>)>,
}
```

`ToolCoordinator::execute_with_prompting` and `spawn_tool_execution` also drop
their `mcp_client` and `root` parameters.

### Changes to `run_turn_loop`

`run_turn_loop` no longer threads `mcp_client` or `root` into `ToolCoordinator`,
because the executor source already holds the constructed runtimes.

`root` is removed from `run_turn_loop` entirely.
`ToolRenderer`, which uses `root` to run custom argument-formatting commands,
receives `root` directly at construction time in `jp_cli` (or via
`StdioRuntime`), keeping terminal rendering separate from tool execution.

`mcp_client` remains a parameter on `run_turn_loop` because the inquiry backend
still uses it to resolve tool definitions for LLM-answered tool questions, but
it no longer flows through the tool execution path.

### Unchanged Types and Behavior

- **`ToolSource`**: Remains a string-based enum (`builtin`, `local`,
  `mcp.<server>.<tool>`) that describes where a tool's definition comes from.
- **`ToolDefinition`**: Continues to describe a tool's name, docs, and
  parameters.
- **`ExecutionOutcome`**: Remains the return type of `ToolRuntime::execute()`.
- **`ToolConfigWithDefaults`**: Carries per-tool configuration as before, with
  the addition of the `runtime` field.
- **Tool behavior**: Existing tools produce identical results before and after
  this refactor.

### Crate Boundaries

`ToolRuntime` and `ToolRuntimeKind` live in `jp_llm` alongside `ToolDefinition`
and `ExecutionOutcome`, keeping all tool execution abstractions in one crate.

The three initial runtime implementations (`StdioRuntime`, `McpRuntime`, and
`BuiltinRuntime`) also live in `jp_llm`, which already houses their underlying
execution helpers (`run_tool_command`, `execute_mcp`, and `execute_builtin`).

`jp_cli` owns the workspace root, the MCP client, and the builtin executor
registry, so it constructs the runtime instances and passes them into the
resolution layer.

## Drawbacks

- **Indirection.** A `match` dispatch is explicit and easy to step through in a
  debugger.
  A trait object adds a vtable lookup and makes "go to definition" land on the
  trait rather than the concrete implementation.
  For only three implementations, a `match` is simpler.

- **Upfront runtime construction.** `jp query` creates all runtime instances at
  startup even if a session invokes no tools of a given kind.
  The overhead is small because each runtime is a thin struct wrapping an `Arc`
  or path, but unused instances are still allocated.

- **Unimplemented `runtime` values in configuration.** `ToolConfig` accepts
  `"vfs"` and `"wasm"` before those runtimes exist, which could confuse users
  who configure them early.
  Returning a clear unsupported-runtime error mitigates this.

## Alternatives

### Keep the `match` dispatch

Leave the three-way `match` in `ToolDefinition::execute()` and add arms as new
execution mechanisms arrive.

Rejected because every runtime's dependencies would still have to be threaded
through `run_turn_loop()`, `ToolCoordinator::execute_with_prompting()`,
`spawn_tool_execution()`, and `Executor::execute()`.
Adding WASM and VFS dependencies would widen those parameter lists further.

### Put `runtime` into `ToolSource`

Replace `ToolSource::Local` with `ToolSource::Stdio`, `ToolSource::Vfs`, and
`ToolSource::Wasm` so the source enum determines the execution mechanism
directly.

Rejected because definition source and execution mechanism are orthogonal.
`ToolSource` answers where a tool's definition comes from (local config, MCP
server, hardcoded builtin), whereas `runtime` answers how it executes
(subprocess, IPC, WASM).
A local tool can run as a subprocess today and as a WASM component tomorrow
without changing its source.
Combining both axes in one enum prevents this and forces the enum to grow with
the product of sources × runtimes.

### Use an enum instead of a trait

```rust
enum ToolRuntime {
    Stdio(StdioRuntime),
    Mcp(McpRuntime),
    Builtin(BuiltinRuntime),
}
```

Dispatch calls by matching on `ToolRuntime` enum variants instead of dynamic
dispatch.

Rejected because a closed enum cannot be extended with new runtimes (WASM, VFS,
or plugin-provided runtimes once [RFD 016] matures) without modifying `jp_llm`.
The vtable cost of `Arc<dyn ToolRuntime>` is negligible at I/O boundaries.

## Non-Goals

- **Implementing `VfsRuntime` or `WasmRuntime`.** This RFD defines the trait and
  configuration surface they will use; future RFDs implement the runtimes.

- **Changing tool behavior.** Every existing tool produces identical results
  before and after this refactor.

- **Sandboxing.** [RFD 075] addresses tool sandboxing and access policies.
  This RFD does not add security restrictions to tool execution.

- **Stateful tool protocol.** [RFD 009] defines multi-turn tool interaction
  (spawn/fetch/apply/abort).
  `ToolRuntime` covers a single execution step; when [RFD 009] is implemented,
  the coordinator will manage state transitions above the runtime layer.

- **Self-describing tools.** [RFD D06] and [RFD D07] cover how tool schemas are
  authored and discovered, which is orthogonal to how tools execute.

## Risks and Open Questions

### `ToolRenderer` and `root`

`ToolRenderer` in `jp_cli` spawns a subprocess for custom argument formatting
and needs the workspace `root`.
Because `root` is no longer threaded through `run_turn_loop`, `ToolRenderer`
must receive `root` at construction time instead of through the execution path.

### Inquiry backend and `mcp_client`

The inquiry backend uses `mcp_client` to resolve tool definitions for
LLM-answered tool questions.
Because inquiry resolution is separate from tool execution, `mcp_client` remains
a parameter on `run_turn_loop` even after the tool execution path stops using
it.

### `config` parameter on `ToolRuntime::execute()`

`ToolRuntime::execute()` receives `&ToolConfigWithDefaults`, which carries the
full tool configuration (`command`, `source`, `parameters`, `options`).
Each runtime reads only the fields it needs (`command` for `StdioRuntime`,
nothing for `BuiltinRuntime`) and ignores the rest.
Passing only runtime-specific config would require per-runtime extraction logic
that adds complexity without simplifying callers.

### Migration of `execute_local` internals

`StdioRuntime::execute()` takes over parameter defaults, argument validation,
context JSON construction, and command execution from
`ToolDefinition::execute_local()`.
The logic is unchanged, but moving it risks introducing regressions.
The existing test suite (`tool_tests.rs`, `turn_loop_tests.rs`,
`coordinator_tests.rs`) covers these paths and must pass unchanged.

## Implementation Plan

### Phase 1: Define `ToolRuntime` trait and `ToolRuntimeKind` config

Add the `ToolRuntime` trait to `jp_llm`, and add the `ToolRuntimeKind` enum and
`runtime` field to `ToolConfig` in `jp_config` with config parsing,
serialization, and `.wasm` inference.

**Depends on:** Nothing.
**Mergeable:** Yes.

### Phase 2: Implement `StdioRuntime`

Move `execute_local` from `ToolDefinition` into `StdioRuntime::execute()`,
preserving parameter defaults, argument validation, context construction,
subprocess spawning, and output parsing.

**Depends on:** Phase 1.
**Mergeable:** Yes.

### Phase 3: Implement `McpRuntime` and `BuiltinRuntime`

Move `execute_mcp` into `McpRuntime` and `execute_builtin` into
`BuiltinRuntime`.

**Depends on:** Phase 1.
**Mergeable:** Yes (parallel with Phase 2).

### Phase 4: Update `Executor` trait and `ToolCoordinator`

Remove `mcp_client` and `root` from `Executor::execute()`, `ToolCoordinator`,
and `spawn_tool_execution`.
Update `ToolExecutor` to hold `Arc<dyn ToolRuntime>` and
`TerminalExecutorSource` to accept runtime-paired definitions.

**Depends on:** Phases 2 and 3.
**Mergeable:** Yes.

### Phase 5: Update `run_turn_loop` and `handle_turn`

Remove `root` from `run_turn_loop` and pass it to `ToolRenderer` at
construction.
Update `handle_turn` in `jp_cli` to construct the runtimes and pass them into
the resolution layer.

**Depends on:** Phase 4.
**Mergeable:** Yes.

### Phase 6: Update `MockExecutor` and `TestExecutorSource`

Remove `mcp_client` and `root` from `MockExecutor::execute()` and
`TestExecutorSource`, and update test helpers and assertions so all existing
tests pass unchanged.

**Depends on:** Phase 4.
**Mergeable:** Yes (parallel with Phase 5).

### Phase 7: Remove `ToolDefinition::execute()` dispatch

Remove `execute()`, `execute_local`, `execute_mcp`, and `execute_builtin` from
`ToolDefinition` once all call sites use `ToolRuntime`.

**Depends on:** Phases 5 and 6.
**Mergeable:** Yes.

## References

- [RFD 009]: Stateful tool protocol.
  Defines multi-turn tool interaction; `ToolRuntime` covers single-execution
  steps within that model.
- [RFD 016]: WASM plugin architecture.
  A future `WasmRuntime` will implement `ToolRuntime` using the WASM plugin
  infrastructure.
- [RFD 075]: Tool sandbox and access policy.
  Sandboxing is orthogonal to the runtime model; runtime implementations apply
  sandbox profiles (for example, `StdioRuntime` applies `sandbox-exec` before
  spawning).
- [RFD D06]: Self-describing local tools.
  Schema discovery happens at resolution time before the runtime is invoked.
- [RFD D07]: Typed tool SDK for Rust.
  SDK improvements affect tool authoring, not execution dispatch.
- [RFD D09]: Project filesystem abstraction.
  Replaces `StdioRuntime`'s `root` path with `Arc<dyn ProjectFiles>`.

[RFD 009]: ../009-stateful-tool-protocol.md
[RFD 016]: ../016-wasm-plugin-architecture.md
[RFD 075]: ../075-tool-sandbox-and-access-policy.md
[RFD D06]: D06-self-describing-local-tools.md
[RFD D07]: D07-typed-tool-sdk-for-rust.md
[RFD D09]: D09-project-filesystem-abstraction.md
[RFD D11]: D11-vfs-tool-protocol.md
