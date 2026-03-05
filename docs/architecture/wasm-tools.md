# Wasm Tools Architecture

This document describes the architecture for executing JP tools as
WebAssembly (Wasm) components. It covers the runtime, the host-guest
contract, builtin tools (embedded in the binary), and local Wasm
tools (loaded from disk).

Related: [Knowledge Base Architecture](knowledge-base.md) — the
`learn` tool is the first builtin Wasm tool.

## Table of Contents

- [Overview](#overview)
- [Design Goals](#design-goals)
- [Runtime Selection](#runtime-selection)
- [Host-Guest Contract](#host-guest-contract)
  - [WIT Interface](#wit-interface)
  - [Mapping to `jp_tool` Types](#mapping-to-jp_tool-types)
  - [WASI Capabilities](#wasi-capabilities)
- [Builtin Wasm Tools](#builtin-wasm-tools)
- [Local Wasm Tools](#local-wasm-tools)
- [The `learn` Tool](#the-learn-tool)
- [Test Tool](#test-tool)
- [Crate Structure](#crate-structure)
- [Data Flow](#data-flow)
- [Error Handling](#error-handling)
- [Testing Strategy](#testing-strategy)
- [Migration Path](#migration-path)

---

## Overview

JP currently supports two tool execution models:

1. **Local tools** — shell commands spawned as subprocesses
   (`ToolSource::Local`)
2. **MCP tools** — remote calls to MCP servers
   (`ToolSource::Mcp`)

This architecture adds a third model: **Wasm tools** — sandboxed
WebAssembly components executed in-process via a Wasm runtime. Wasm
tools serve two purposes:

- **Builtin tools** (`ToolSource::Builtin`): Wasm binaries compiled
  into the `jp` binary, loaded via `include_bytes!`. The `learn` tool
  is the first of these.
- **Local Wasm tools** (`ToolSource::Local` with `wasm` option): Wasm
  binaries loaded from disk at runtime. This lets users write custom
  tools as Wasm components without shell command overhead.

Both share the same WIT (Wasm Interface Types) contract, so a single
implementation works in either mode. The only difference is where the
bytes come from.

---

## Design Goals

| Goal | Description |
|------|-------------|
| **Single contract** | One WIT interface for both builtin and local Wasm tools |
| **Sandboxed execution** | Tools run in a Wasm sandbox with scoped filesystem access |
| **Component model** | Target WASI Preview 2 for typed interfaces and future features |
| **Minimal host coupling** | Guest tools depend only on `jp_tool` types and WIT bindings |
| **Lazy loading** | Local Wasm tools are compiled on first use, not at startup |
| **Familiar types** | WIT types mirror the existing `jp_tool::Outcome`, `Context`, etc. |

---

## Runtime Selection

### Decision: `wasmtime`

The Wasm runtime is `wasmtime`. This is a hard requirement driven
by the choice to target WASI Preview 2 (component model).

| Runtime | WASI P1 | WASI P2 / Component Model | Binary overhead |
|---------|---------|---------------------------|-----------------|
| `wasmtime` | Yes | **Yes** | ~15-20 MB |
| `wasmi` | Yes | No | ~1 MB |
| `wasm3` | Partial | No | ~0.5 MB |

`wasmi` was considered for its smaller binary footprint, but it does
not support the component model. Since the project intends to expand
Wasm-based extensibility to other areas (e.g., LLM provider plugins),
investing in `wasip2` from the start avoids a future migration.

### Binary Size Mitigation

The `wasmtime` dependency adds significant binary size. Strategies to
reduce it:

1. **Feature gating**: Put `wasmtime` behind a cargo feature
   (`wasm-tools`), disabled by default for minimal builds.
2. **Cranelift tuning**: `wasmtime` uses cranelift for JIT
   compilation. The `cranelift` feature can be replaced with
   interpreter-only mode (experimental) for smaller binaries at the
   cost of execution speed.
3. **LTO and stripping**: Standard release optimizations
   (`lto = true`, `strip = true`) reduce the overhead.

For the initial implementation, accept the full `wasmtime` dependency
with default features. Optimize later based on real-world binary size
measurements.

### Compilation Target

Guest crates (tools written in Rust) target `wasm32-wasip2`:

```bash
rustup target add wasm32-wasip2
cargo build --target wasm32-wasip2 --release
```

This produces a Wasm component (`.wasm` file) that exports the
functions defined in the WIT interface and can use WASI Preview 2
APIs (filesystem, clocks, random, etc.).

---

## Host-Guest Contract

### WIT Interface

The tool contract is defined as a WIT (Wasm Interface Types) package.
This file lives in the `jp_tool` crate and is shared between host and
guest.

```wit
// wit/tool.wit

package jp:tool@0.1.0;

interface types {
    /// The action requested by the host.
    enum action {
        /// Execute the tool.
        run,
        /// Format the tool call arguments for display.
        format-arguments,
    }

    /// Execution context provided by the host.
    record context {
        /// Working directory (absolute path). For builtin tools,
        /// this is the resolved subjects directory or workspace
        /// root. For local tools, this is the workspace root.
        root: string,

        /// The action to perform.
        action: action,
    }

    /// Structured error information.
    record error-info {
        /// Human-readable error message.
        message: string,

        /// Error chain (source errors).
        trace: list<string>,

        /// Whether the error is transient and the tool call can
        /// be retried.
        transient: bool,
    }

    /// A question the tool needs answered before continuing.
    record question {
        /// Unique question ID. Passed back in `answers` when
        /// the host provides the answer.
        id: string,

        /// The question text to present.
        text: string,

        /// Expected answer type. One of:
        /// - "boolean" (yes/no)
        /// - "text" (free-form)
        /// - JSON object with "select" key and "options" array
        answer-type: string,

        /// Optional default answer (JSON-encoded).
        default: option<string>,
    }

    /// The result of a tool execution.
    variant outcome {
        /// Tool succeeded. Contains the output content.
        success(string),

        /// Tool failed. Contains structured error info.
        error(error-info),

        /// Tool needs additional input. Contains the question.
        needs-input(question),
    }
}

world tool {
    use types.{context, outcome};

    /// Execute the tool.
    ///
    /// Arguments:
    /// - ctx: execution context
    /// - name: the tool name (for multi-tool binaries)
    /// - arguments: JSON-encoded tool arguments
    /// - answers: JSON-encoded answers to previous questions
    ///
    /// Returns the tool outcome.
    export run: func(
        ctx: context,
        name: string,
        arguments: string,
        answers: string,
    ) -> outcome;
}
```

### Mapping to `jp_tool` Types

The WIT types mirror the existing Rust types in `jp_tool`:

| WIT Type | Rust Type (`jp_tool`) |
|----------|----------------------|
| `context` | `Context` |
| `action` | `Action` |
| `outcome::success` | `Outcome::Success` |
| `outcome::error` | `Outcome::Error` |
| `outcome::needs-input` | `Outcome::NeedsInput` |
| `question` | `Question` |
| `error-info` | (fields of `Outcome::Error`) |

The `answer-type` field is serialized as a string rather than a WIT
variant to keep the interface simple. The guest and host both parse
it the same way:

- `"boolean"` → `AnswerType::Boolean`
- `"text"` → `AnswerType::Text`
- `{"select": {"options": [...]}}` → `AnswerType::Select`

The `arguments` and `answers` parameters are JSON strings because
tool arguments are dynamic (schema varies per tool). Parsing happens
inside the guest.

### WASI Capabilities

Wasm tools run in a sandbox. The host grants specific capabilities
via WASI preopens and configuration:

| Capability | Builtin tools | Local Wasm tools |
|------------|---------------|------------------|
| Filesystem (read) | Scoped to topic directory | Scoped to workspace root |
| Filesystem (write) | Denied | Scoped to workspace root |
| Network | Denied | Denied (for now) |
| Environment vars | Denied | Denied |
| Clocks | Allowed | Allowed |
| Random | Allowed | Allowed |
| Stdout/Stderr | Captured by host | Captured by host |

**Filesystem scoping** uses WASI preopened directories. The host
opens a directory and maps it into the guest's filesystem namespace.
The guest can only access files within preopened directories — there
is no escape from the sandbox.

```rust
// Host-side setup (pseudo-code)
let mut wasi = WasiCtxBuilder::new();

// For the `learn` tool: preopen the topic's subjects directory
wasi.preopened_dir(
    &subjects_dir,    // host path: /path/to/.jp/kb/skills
    "/subjects",      // guest path: /subjects
    DirPerms::READ,
    FilePerms::READ,
);

// For local Wasm tools: preopen workspace root
wasi.preopened_dir(
    &workspace_root,
    "/workspace",
    DirPerms::all(),
    FilePerms::all(),
);
```

The guest sees a virtual filesystem:
- Builtin `learn` tool: `/subjects/ast-grep.md`, etc.
- Local Wasm tool: `/workspace/src/main.rs`, etc.

---

## Builtin Wasm Tools

Builtin tools are Wasm binaries compiled into the `jp` binary as
static byte arrays.

### Embedding

```rust
// jp_llm/src/tool.rs (or a new jp_wasm crate)

/// Embedded Wasm binary for the `learn` tool.
const LEARN_WASM: &[u8] = include_bytes!(
    concat!(env!("OUT_DIR"), "/jp_tool_learn.wasm")
);
```

The Wasm binary is built during `cargo build` via a build script
that compiles the guest crate:

```rust
// build.rs
fn main() {
    // Compile jp_tool_learn for wasm32-wasip2
    let status = std::process::Command::new("cargo")
        .args([
            "build",
            "--package", "jp_tool_learn",
            "--target", "wasm32-wasip2",
            "--release",
        ])
        .status()
        .expect("failed to compile learn tool");

    assert!(status.success());

    // Copy the artifact to OUT_DIR
    let wasm_path = "target/wasm32-wasip2/release/jp_tool_learn.wasm";
    let out_dir = std::env::var("OUT_DIR").unwrap();
    std::fs::copy(wasm_path, format!("{out_dir}/jp_tool_learn.wasm"))
        .expect("failed to copy wasm binary");

    println!("cargo:rerun-if-changed=crates/jp_tool_learn/src");
}
```

### Execution

When `ToolDefinition::execute()` encounters `ToolSource::Builtin`:

```rust
ToolSource::Builtin { tool } => {
    let tool_name = tool.as_deref().unwrap_or(&self.name);
    let wasm_bytes = match tool_name {
        "learn" => LEARN_WASM,
        _ => return Err(ToolError::UnknownBuiltin(tool_name.into())),
    };

    execute_wasm(
        wasm_bytes,
        tool_name,
        arguments,
        answers,
        context,
        wasi_config,
    ).await
}
```

### Caching

`wasmtime` compiles Wasm bytes to native code on first load. This
compilation is cached:

```rust
use wasmtime::{Engine, component::Component};

// Engine is created once (at startup or lazily)
static ENGINE: LazyLock<Engine> = LazyLock::new(|| {
    let mut config = wasmtime::Config::new();
    config.wasm_component_model(true);
    Engine::new(&config).expect("failed to create wasm engine")
});

// Component is compiled once per Wasm binary
static LEARN_COMPONENT: LazyLock<Component> = LazyLock::new(|| {
    Component::from_binary(&ENGINE, LEARN_WASM)
        .expect("failed to compile learn component")
});
```

For local Wasm tools, the compiled component is cached in a
`HashMap<PathBuf, Component>` keyed by the Wasm file path. This
means the first invocation of a local Wasm tool pays the compilation
cost, but subsequent calls reuse the cached component.

---

## Local Wasm Tools

Local Wasm tools are loaded from disk. They use the same WIT contract
as builtin tools but are configured via `ToolConfig`.

### Configuration

A new optional `wasm` field on `ToolConfig`:

```toml
[conversation.tools.my_custom_tool]
source = "local"
wasm = ".jp/tools/my_custom_tool.wasm"
description = "A custom Wasm-based tool"
parameters.input.type = "string"
parameters.input.required = true
```

When `wasm` is present on a `local` tool, the Wasm binary is loaded
and executed instead of spawning a shell command. The `command` field
is ignored.

### Rust Type Change

```rust
// jp_config/src/conversation/tool.rs

pub struct ToolConfig {
    // ...existing fields...

    /// Path to a Wasm binary for local tools.
    ///
    /// When set on a `local` tool, the tool is executed as a Wasm
    /// component instead of a shell command. The path is relative
    /// to the workspace root.
    ///
    /// Ignored for `builtin` and `mcp` tools.
    pub wasm: Option<RelativePathBuf>,
}
```

### Execution Path

In `ToolDefinition::execute()`, the `Local` branch checks for a
Wasm binary:

```rust
ToolSource::Local { tool } => {
    if let Some(wasm_path) = config.wasm() {
        // Load and execute Wasm component
        let abs_path = root.join(wasm_path);
        execute_wasm_from_path(
            &abs_path,
            tool_name,
            arguments,
            answers,
            context,
            wasi_config,
        ).await
    } else {
        // Existing shell command execution
        self.execute_local(/* ... */).await
    }
}
```

### Lazy Loading

Local Wasm tools are compiled on first invocation. The compiled
`Component` is cached for the duration of the process. This avoids
paying compilation cost for tools that are never called.

```rust
// Pseudo-code
fn execute_wasm_from_path(path, ...) {
    let component = COMPONENT_CACHE
        .entry(path.to_owned())
        .or_insert_with(|| {
            let bytes = std::fs::read(path)?;
            Component::from_binary(&ENGINE, &bytes)?
        });

    execute_wasm_component(component, ...)
}
```

---

## The `learn` Tool

The `learn` tool is the first builtin Wasm tool. It is the primary
driver for the Wasm tools infrastructure.

### Guest Crate: `jp_tool_learn`

Located at `crates/jp_tool_learn/`. Targets `wasm32-wasip2`.

**Dependencies:**

```toml
[package]
name = "jp_tool_learn"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = "0.41"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
glob = "0.3"
```

**Implementation outline:**

```rust
// crates/jp_tool_learn/src/lib.rs

wit_bindgen::generate!({
    world: "tool",
    path: "../jp_tool/wit/tool.wit",
});

struct LearnTool;

impl Guest for LearnTool {
    fn run(
        ctx: Context,
        name: String,
        arguments: String,
        answers: String,
    ) -> Outcome {
        let args: LearnArgs = match serde_json::from_str(&arguments) {
            Ok(v) => v,
            Err(e) => return Outcome::Error(ErrorInfo {
                message: format!("Invalid arguments: {e}"),
                trace: vec![],
                transient: false,
            }),
        };

        match ctx.action {
            Action::Run => execute_learn(&ctx, &args),
            Action::FormatArguments => format_args(&args),
        }
    }
}

export!(LearnTool);

#[derive(serde::Deserialize)]
struct LearnArgs {
    /// Subjects to load (glob patterns).
    subjects: Option<SubjectsArg>,

    /// Topic metadata injected by the host.
    #[serde(default)]
    _topic: Option<TopicMeta>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum SubjectsArg {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(serde::Deserialize)]
struct TopicMeta {
    title: Option<String>,
    description: Option<String>,
    disabled: Vec<String>,
    learned: Vec<String>,
}

fn execute_learn(ctx: &Context, args: &LearnArgs) -> Outcome {
    // The host preopens the topic's subjects directory at
    // /subjects. The guest reads from there.
    let subjects_dir = "/subjects";

    match &args.subjects {
        None => list_subjects(subjects_dir, args),
        Some(patterns) => load_subjects(subjects_dir, patterns, args),
    }
}
```

### Host-Side Preparation

Before calling the Wasm guest, the host:

1. Resolves the topic from the LLM's arguments
2. Preopens the topic's subjects directory with read-only access
3. Injects topic metadata (`_topic`) into the arguments

```rust
// Pseudo-code in ToolDefinition::execute() for builtin "learn"

let topic_id = resolve_topic(&kb_config, &arguments["topic"])?;
let topic = &kb_config.topics[&topic_id];
let subjects_dir = workspace_root.join(&topic.subjects);

// Inject topic metadata
let mut args = arguments.clone();
args.insert("_topic", json!({
    "title": topic.title,
    "description": topic.description,
    "disabled": topic.disabled,
    "learned": topic.learned,
}));

// Configure WASI with scoped filesystem
let wasi = WasiCtxBuilder::new()
    .preopened_dir(&subjects_dir, "/subjects", READ, READ)
    .build();

execute_wasm(LEARN_WASM, "learn", &args, answers, context, wasi)
```

---

## Test Tool

A simple Wasm tool for validating the infrastructure. This tool is
NOT compiled into the `jp` binary. It is loaded from disk via the
`wasm` config option.

### Purpose

Verify that:

1. Local Wasm tools load and execute correctly
2. The WIT contract works end-to-end
3. WASI filesystem scoping works (read/write)
4. The same contract works for both builtin and local tools

### Implementation

Located at `crates/jp_tool_test/`. A minimal tool that reads a file
and returns its content:

```rust
// crates/jp_tool_test/src/lib.rs

wit_bindgen::generate!({
    world: "tool",
    path: "../jp_tool/wit/tool.wit",
});

struct TestTool;

impl Guest for TestTool {
    fn run(
        ctx: Context,
        _name: String,
        arguments: String,
        _answers: String,
    ) -> Outcome {
        let args: TestArgs = match serde_json::from_str(&arguments) {
            Ok(v) => v,
            Err(e) => return Outcome::Error(ErrorInfo {
                message: format!("Invalid arguments: {e}"),
                trace: vec![],
                transient: false,
            }),
        };

        // Read a file from the preopened workspace directory
        match std::fs::read_to_string(
            format!("/workspace/{}", args.path)
        ) {
            Ok(content) => Outcome::Success(content),
            Err(e) => Outcome::Error(ErrorInfo {
                message: format!("Failed to read file: {e}"),
                trace: vec![],
                transient: false,
            }),
        }
    }
}

export!(TestTool);

#[derive(serde::Deserialize)]
struct TestArgs {
    path: String,
}
```

### Configuration

```toml
# .jp/config.toml (test workspace)
[conversation.tools.wasm_test]
source = "local"
wasm = ".jp/tools/test_tool.wasm"
description = "Test tool that reads a file from the workspace"

[conversation.tools.wasm_test.parameters.path]
type = "string"
description = "Relative file path to read"
required = true
```

### Test Cases

```rust
#[tokio::test]
async fn test_local_wasm_tool_reads_file() {
    // 1. Set up workspace with a test file
    // 2. Compile jp_tool_test to wasm32-wasip2
    // 3. Configure tool with wasm path
    // 4. Execute tool via ToolDefinition::execute()
    // 5. Assert file content is returned
}

#[tokio::test]
async fn test_wasm_filesystem_scoping() {
    // Verify the guest cannot read files outside the preopened dir
}

#[tokio::test]
async fn test_builtin_and_local_same_contract() {
    // Run the same test against a builtin tool (learn) and a
    // local Wasm tool (test), verifying both produce valid
    // Outcome values
}
```

---

## Crate Structure

```
crates/
├── jp_tool/                    # Shared types + WIT definition
│   ├── src/lib.rs              # Outcome, Context, Question, etc.
│   └── wit/
│       └── tool.wit            # WIT interface definition
│
├── jp_tool_learn/              # Learn tool (Wasm guest)
│   ├── Cargo.toml              # target = wasm32-wasip2
│   └── src/lib.rs              # Implements WIT `run` export
│
├── jp_tool_test/               # Test tool (Wasm guest)
│   ├── Cargo.toml              # target = wasm32-wasip2
│   └── src/lib.rs              # Minimal file-read tool
│
├── jp_wasm/                    # Wasm runtime host (new crate)
│   ├── Cargo.toml              # depends on wasmtime
│   └── src/
│       ├── lib.rs              # Public API
│       ├── engine.rs           # Engine + component caching
│       ├── host.rs             # WASI configuration, preopens
│       └── execute.rs          # Component instantiation + call
│
├── jp_llm/                     # Existing — tool execution
│   └── src/tool.rs             # ToolDefinition::execute() routes
│                               # Builtin → jp_wasm
│
└── jp_config/                  # Existing — tool configuration
    └── src/conversation/tool.rs # ToolConfig gains `wasm` field
```

### `jp_wasm` Crate

A new crate that encapsulates all `wasmtime` interaction. No other
crate depends on `wasmtime` directly.

```rust
// jp_wasm/src/lib.rs

/// Execute a Wasm component from raw bytes.
///
/// The component must export the `run` function defined in
/// the `jp:tool` WIT interface.
pub async fn execute(
    bytes: &[u8],
    name: &str,
    arguments: &str,
    answers: &str,
    context: jp_tool::Context,
    wasi_config: WasiConfig,
) -> Result<jp_tool::Outcome, Error> {
    // ...
}

/// Execute a Wasm component from a file path.
///
/// Caches the compiled component for subsequent calls.
pub async fn execute_from_path(
    path: &Path,
    name: &str,
    arguments: &str,
    answers: &str,
    context: jp_tool::Context,
    wasi_config: WasiConfig,
) -> Result<jp_tool::Outcome, Error> {
    // ...
}

/// WASI capability configuration for a tool invocation.
pub struct WasiConfig {
    /// Directories to preopen (guest path → host path, permissions).
    pub preopens: Vec<Preopen>,
}

pub struct Preopen {
    pub guest_path: String,
    pub host_path: PathBuf,
    pub dir_perms: DirPerms,
    pub file_perms: FilePerms,
}

pub enum DirPerms { Read, ReadWrite }
pub enum FilePerms { Read, ReadWrite }
```

### Dependency Graph

```
jp_cli
  ├── jp_llm
  │    ├── jp_wasm (new)        ← wasmtime dependency
  │    │    └── jp_tool         ← shared types + WIT
  │    └── jp_tool
  ├── jp_config
  │    └── (no wasm dependency)
  └── jp_tool

jp_tool_learn (wasm guest)
  ├── wit-bindgen              ← generates WIT exports
  ├── serde, serde_json
  └── glob

jp_tool_test (wasm guest)
  ├── wit-bindgen
  └── serde, serde_json
```

Key: `wasmtime` is isolated in `jp_wasm`. Guest crates do not depend
on `wasmtime` — they use `wit-bindgen` to generate Wasm exports.

---

## Data Flow

### Builtin Tool Execution (learn)

```
LLM calls: learn(topic: "skills", subjects: ["ast-grep"])
     │
     ▼
ToolDefinition::execute()
     │
     ├── source = ToolSource::Builtin { tool: "learn" }
     │
     ├── Host resolves topic:
     │   └── "skills" → TopicConfig { subjects: ".jp/kb/skills", ... }
     │
     ├── Host injects _topic metadata into arguments
     │
     ├── Host builds WasiConfig:
     │   └── preopen: /subjects → .jp/kb/skills (read-only)
     │
     ▼
jp_wasm::execute(LEARN_WASM, "learn", args, answers, ctx, wasi)
     │
     ├── Get cached Component (or compile from bytes)
     ├── Build WASI context with preopens
     ├── Instantiate component
     ├── Call exported `run` function
     │
     ▼
Wasm Guest (jp_tool_learn)
     │
     ├── Parse arguments (subjects: ["ast-grep"])
     ├── Read /subjects/ directory (WASI filesystem)
     ├── Match "ast-grep" → ast-grep.md
     ├── Read file content
     ├── Apply format handling (markdown → pass-through)
     │
     └── Return Outcome::Success("...file content...")
     │
     ▼
jp_wasm converts WIT outcome → jp_tool::Outcome
     │
     ▼
ToolDefinition::execute() → ExecutionOutcome::Completed
     │
     ▼
Tool result returned to LLM
```

### Local Wasm Tool Execution

```
LLM calls: wasm_test(path: "src/main.rs")
     │
     ▼
ToolDefinition::execute()
     │
     ├── source = ToolSource::Local, config.wasm = Some(".jp/tools/test.wasm")
     │
     ├── Host builds WasiConfig:
     │   └── preopen: /workspace → workspace root (read-write)
     │
     ▼
jp_wasm::execute_from_path(".jp/tools/test.wasm", ...)
     │
     ├── Check component cache
     │   ├── Cache hit → reuse compiled Component
     │   └── Cache miss → read bytes, compile, cache
     │
     ├── Build WASI context with preopens
     ├── Instantiate component
     ├── Call exported `run` function
     │
     ▼
Wasm Guest (jp_tool_test)
     │
     ├── Parse arguments (path: "src/main.rs")
     ├── Read /workspace/src/main.rs (WASI filesystem)
     │
     └── Return Outcome::Success("fn main() { ... }")
     │
     ▼
jp_wasm converts WIT outcome → jp_tool::Outcome
     │
     ▼
ExecutionOutcome::Completed { result: Ok("fn main() { ... }") }
```

---

## Error Handling

### Guest Errors

Tool-level errors are returned as `Outcome::Error` or
`Outcome::NeedsInput`. These propagate through the normal tool
result path — the LLM receives them and can respond appropriately.

### Runtime Errors

Wasm runtime errors (compilation failure, instantiation failure,
trap) are infrastructure errors. They are returned as
`Err(ToolError)` from `ToolDefinition::execute()`:

```rust
pub enum ToolError {
    // ...existing variants...

    /// Wasm compilation failed.
    #[error("Failed to compile Wasm component: {0}")]
    WasmCompilation(String),

    /// Wasm instantiation failed.
    #[error("Failed to instantiate Wasm component: {0}")]
    WasmInstantiation(String),

    /// Wasm execution trapped.
    #[error("Wasm tool trapped: {0}")]
    WasmTrap(String),

    /// Wasm binary not found at the configured path.
    #[error("Wasm binary not found: {path}")]
    WasmNotFound { path: String },
}
```

### Filesystem Errors

If the guest attempts to access a path outside its preopened
directories, the WASI runtime returns a "permission denied" or
"no such file" error. This is by design — the sandbox is enforced
at the runtime level, not by the guest.

### Cancellation

Wasm execution does not natively support cancellation tokens. For
short-running tools (like `learn`), this is acceptable. For
potentially long-running Wasm tools, the host can:

1. Run the Wasm execution in a `tokio::spawn` task
2. Race it against the cancellation token
3. Drop the task on cancellation (aborts the Wasm instance)

```rust
tokio::select! {
    biased;
    () = cancellation_token.cancelled() => {
        Ok(ExecutionOutcome::Cancelled { id })
    }
    result = execute_wasm(...) => {
        result
    }
}
```

---

## Testing Strategy

### Unit Tests

**WIT type conversion:**

```rust
#[test]
fn test_outcome_roundtrip() {
    // Verify jp_tool::Outcome ↔ WIT outcome conversion
    let outcome = Outcome::Success { content: "hello".into() };
    let wit_outcome = to_wit_outcome(&outcome);
    let roundtrip = from_wit_outcome(wit_outcome);
    assert_eq!(outcome, roundtrip);
}
```

**WASI configuration:**

```rust
#[test]
fn test_wasi_preopen_read_only() {
    let config = WasiConfig {
        preopens: vec![Preopen {
            guest_path: "/subjects".into(),
            host_path: "/tmp/test_subjects".into(),
            dir_perms: DirPerms::Read,
            file_perms: FilePerms::Read,
        }],
    };

    let ctx = build_wasi_context(&config);
    // Verify preopened directory is configured correctly
}
```

### Integration Tests

**Builtin tool (learn):**

```rust
#[tokio::test]
async fn test_learn_tool_lists_subjects() {
    // Set up a temp directory with subject files
    // Execute the learn Wasm component
    // Assert the output lists the subjects
}

#[tokio::test]
async fn test_learn_tool_reads_subject() {
    // Set up a temp directory with a subject file
    // Execute learn with subjects: ["my-subject"]
    // Assert the file content is returned
}

#[tokio::test]
async fn test_learn_tool_hides_dot_prefixed() {
    // Set up subjects including .hidden.md
    // Execute learn with subjects: ["*"]
    // Assert .hidden.md is not in the output
}
```

**Local Wasm tool:**

```rust
#[tokio::test]
async fn test_local_wasm_tool_execution() {
    // Compile jp_tool_test to wasm32-wasip2
    // Configure as a local tool with wasm path
    // Execute and verify result
}

#[tokio::test]
async fn test_wasm_sandbox_prevents_escape() {
    // Preopen /workspace → temp dir
    // Guest tries to read /etc/passwd (or ../../etc/passwd)
    // Assert: error, not file content
}
```

**Contract validation:**

```rust
#[tokio::test]
async fn test_builtin_and_local_produce_same_outcome_type() {
    // Execute the learn tool (builtin)
    // Execute the test tool (local wasm)
    // Both return jp_tool::Outcome — verify they serialize
    // identically through the tool result pipeline
}
```

---

## Migration Path

### Phase 1: WIT Definition and `jp_tool` Changes

1. Create `crates/jp_tool/wit/tool.wit` with the interface definition
2. Add `wit-bindgen` as an optional dependency of `jp_tool` (for
   guest-side use)
3. Verify the WIT types align with existing Rust types
4. No runtime changes — just the contract definition

### Phase 2: `jp_wasm` Crate

1. Create `crates/jp_wasm/` with `wasmtime` dependency
2. Implement `Engine` creation and component caching
3. Implement WASI context builder (preopens, permissions)
4. Implement `execute()` and `execute_from_path()`
5. Implement WIT ↔ `jp_tool` type conversion
6. Add `jp_wasm` to workspace dependencies
7. Unit tests for engine, caching, and type conversion

### Phase 3: `jp_tool_learn` Guest Crate

1. Create `crates/jp_tool_learn/` targeting `wasm32-wasip2`
2. Add `wit-bindgen` dependency, generate exports from WIT
3. Implement subject listing (directory scan, hidden/disabled filter)
4. Implement subject loading (glob matching, file reading)
5. Implement file format handling (pass-through vs fenced)
6. Compile to `.wasm` and verify with `wasmtime` CLI

### Phase 4: Host Integration

1. Add build script to compile `jp_tool_learn` and embed via
   `include_bytes!`
2. Implement `ToolSource::Builtin` in `ToolDefinition::new()` —
   generate `ToolDefinition` from hardcoded metadata (the Wasm guest
   doesn't self-describe its schema; the host provides it)
3. Implement `ToolSource::Builtin` in `ToolDefinition::execute()` —
   delegate to `jp_wasm::execute()`
4. Add WASI filesystem scoping for the `learn` tool (preopen
   subjects directory, read-only)
5. Integration tests: tool call → Wasm execution → result

### Phase 5: `ToolConfig` Wasm Option

1. Add `wasm: Option<RelativePathBuf>` to `ToolConfig`
2. Add partial config, assignment, and delta support
3. Update `ToolSource::Local` execution path to check for `wasm`
4. Implement `jp_wasm::execute_from_path()` with component caching
5. Add config snapshot tests

### Phase 6: Test Tool

1. Create `crates/jp_tool_test/` targeting `wasm32-wasip2`
2. Implement minimal file-read tool
3. Compile to `.wasm`
4. Write integration tests:
   - Local Wasm tool execution
   - Filesystem scoping (sandbox escape prevention)
   - Same contract as builtin tools
5. Add test workspace configuration

### Phase 7: Cleanup

1. Remove `todo!()` from `ToolSource::Builtin` match arms
2. Update `docs/architecture/index.md` with links to new docs
3. Update `docs/features/tools.md` with Wasm tool documentation
4. Audit `wasmtime` feature flags for binary size optimization
