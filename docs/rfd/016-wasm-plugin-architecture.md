# RFD 016: Wasm Plugin Architecture

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-28

## Summary

This RFD defines JP's plugin system. Plugins are Wasm components that extend JP
with new capabilities — attachment handlers, tools, LLM providers, and more. A
plugin exports a required `plugin` interface for identification and any number
of optional capability interfaces. The host discovers capabilities at load time
using `wasmtime`'s dynamic export inspection. All host interaction is mediated
through JP-controlled imports (`jp:host`), sandboxed per-plugin.

## Motivation

JP has several extension points — attachment handlers, tools, LLM providers —
that are currently hardcoded into the binary. Adding a new attachment type or
tool means writing a Rust crate, wiring it into the workspace, and recompiling.
Users only have limited ways to extend JP without forking the project.

We need a plugin system that:

1. Lets third parties extend JP without recompiling.
2. Runs untrusted code safely — plugins should not have unrestricted access to
   the host filesystem, network, or environment.
3. Supports multiple capability types from a single plugin (e.g., a Jira plugin
   that provides both an attachment handler and a tool).
4. Scales to many capability types without combinatorial complexity.
5. Works across platforms and languages.

Wasm with the component model meets all five requirements: sandboxed by default,
cross-platform, multi-language (via `wit-bindgen`), and the component model
provides typed interfaces that can be composed freely.

## Design

### Interface model

A plugin is a Wasm component that exports one or more interfaces from the
`jp:plugin` package. The `plugin` interface is required — it identifies the
plugin. Capability interfaces are optional — the host discovers which ones the
component exports and registers them accordingly.

```wit
package jp:plugin@0.1.0;

/// Required. Every plugin must export this interface.
interface plugin {
    /// A unique identifier for this plugin (e.g. "jira", "bear").
    name: func() -> string;
}

interface types {
    record error {
        message: string,
    }
}
```

Capability interfaces are defined in the same package. Each interface represents
a distinct extension point:

```wit
/// Attachment handler capability.
interface attachment {
    use types.{error};

    record attachment {
        source: string,
        description: option<string>,
        content: string,
    }

    schemes: func() -> list<string>;
    validate: func(uri: string, cwd: string) -> result<_, error>;
    resolve: func(uris: list<string>, cwd: string) -> result<list<attachment>, error>;
}

/// Future capability interfaces follow the same pattern:
/// interface tool { ... }
/// interface llm { ... }
```

JP publishes these interfaces. Plugin authors compose their own world from
whichever interfaces they implement:

```wit
// A plugin author's WIT file for a Jira plugin
// that provides both attachments and tools.
world jira-plugin {
    import jp:host/process;
    import jp:host/http;
    import jp:host/filesystem;

    export jp:plugin/plugin;
    export jp:plugin/attachment;
    // export jp:plugin/tool;  (when the tool interface exists)
}
```

Guest-side `wit-bindgen` generates typed bindings for the chosen world. The
plugin only implements the interfaces it exports — no stubs, no unused code.

JP also provides convenience worlds for common cases:

```wit
/// For plugins that only provide attachment handling.
world attachment-plugin {
    import jp:host/process;
    import jp:host/http;
    import jp:host/filesystem;

    export plugin;
    export attachment;
}
```

These convenience worlds are optional shortcuts — plugin authors can always
define their own.

### Capability discovery

WIT does not support optional exports (yet) - a component must implement all
exports defined in its world. But the host does not need to know the guest's
world. The host uses `wasmtime`'s runtime API to probe which interfaces a
component actually exports.

At load time, the host:

1. Instantiates the component, providing all `jp:host/*` imports via the
   `Linker`.
2. Calls `instance.get_export_index(None, "jp:plugin/plugin")`. If absent, the
   component is not a valid plugin — error.
3. Calls `plugin.name()` to identify the plugin.
4. Probes for each known capability interface:
   - `instance.get_export_index(None, "jp:plugin/attachment")` — if present,
     register as an attachment handler.
   - `instance.get_export_index(None, "jp:plugin/tool")` — if present, register
     as a tool provider.
   - (and so on for future capability types)

This approach scales to any number of capability types. Adding a new capability
means defining a new WIT interface and adding one probe call on the host side.
Existing plugins are unaffected — they don't export the new interface and the
host simply skips it.

The trade-off is that the host uses `wasmtime`'s dynamic API (`get_func`,
`get_typed_func`) rather than `bindgen!`-generated static bindings. This means
slightly more boilerplate on the host side and runtime type assertions instead
of compile-time checks. The cost is paid once in `jp_wasm` and does not grow
with plugin count or capability count.

### Host imports

Plugins do not use WASI capabilities directly. All host interaction goes through
JP's own imports under the `jp:host` package. This gives the host full control:
every call is checked against the plugin's sandbox configuration before
executing.

```wit
package jp:host@0.1.0;

/// Run commands on the host system.
///
/// The host checks the plugin's sandbox config before executing.
/// Denied commands return an error, not a failed exit code.
///
/// Subprocesses run with a clean environment. Only variables listed
/// in `envs` — and allowed by the plugin's sandbox config — are
/// forwarded from the host process. The handler never sees the values.
interface process {
    record command-output {
        stdout: list<u8>,
        stderr: list<u8>,
        exit-code: s32,
    }

    run: func(
        program: string,
        args: list<string>,
        cwd: string,
        envs: list<string>,
    ) -> result<command-output, string>;
}

/// Make outbound HTTP requests.
///
/// The host checks the plugin's sandbox config before connecting.
/// Denied URLs return an error.
///
/// Header values support `${VAR}` substitution: the host replaces
/// `${VAR}` with the value of `VAR` from its own environment, if
/// `VAR` is listed in the plugin's `network.envs` config. Unknown
/// or disallowed variables cause an error. The plugin never sees
/// the resolved values.
interface http {
    record http-header {
        name: string,
        value: string,
    }

    record http-response {
        status: u16,
        body: list<u8>,
    }

    get: func(url: string, headers: list<http-header>) -> result<http-response, string>;
}

/// Read files and directories on the host filesystem.
///
/// Paths are resolved relative to the workspace root.
/// The host checks the plugin's sandbox config before reading.
interface filesystem {
    record file-metadata {
        is-file: bool,
        is-dir: bool,
        size: u64,
    }

    read: func(path: string) -> result<list<u8>, string>;
    list-dir: func(path: string) -> result<list<string>, string>;
    metadata: func(path: string) -> result<file-metadata, string>;
}
```

Guests import only what they need. A plugin that just parses URLs and transforms
data doesn't import anything. A plugin that runs `git` commands imports
`jp:host/process`. A plugin that calls an API imports `jp:host/http`.

The interfaces are intentionally narrow. `process.run` is "run a command in a
clean environment, get output" — not raw `fork`/`exec`. `http.get` is a simple
GET with optional headers — not a full HTTP client. The interfaces can be
extended later (e.g. `http.post`, `process.run_streaming`) as needs arise.

### Plugin configuration

Plugins are configured as a top-level `plugins` array. The simplest form is a
list of Wasm paths:

```toml
plugins = ["simple_plugin.wasm", "another_plugin.wasm"]
```

Plugins that need sandbox configuration use the array-of-tables form:

```toml
[[plugins]]
wasm = ".jp/plugins/jira.wasm"

[plugins.sandbox.network]
allow = ["https://jira.example.com"]
envs = ["JIRA_API_TOKEN"]

[[plugins]]
wasm = "~/.jp/plugins/notion.wasm"
```

The plugin identifies itself via `plugin.name()` — there is no name key in the
config. The `wasm` key specifies the component path. The plugin format is
inferred from context (currently always Wasm; other formats may be supported in
the future).

If two plugins return the same name from `plugin.name()`, the host errors at
startup with a clear message identifying both.

Components are compiled on first use and cached for the process lifetime.

### Wasm runtime

The Wasm runtime is [`wasmtime`](https://github.com/bytecodealliance/wasmtime)
with the WASI Preview 2 component model. Guest bindings are generated via
[`wit-bindgen`](https://github.com/bytecodealliance/wit-bindgen).

WIT provides a better experience for plugin authors compared to a custom ABI:

- **Typed contracts.** WIT definitions are the interface documentation and the
  code generation source. Errors surface at compile time, not runtime.
- **Multi-language support.** `wit-bindgen` generates idiomatic bindings for
  Rust, Go, Python, C, and others. Plugin authors get generated glue code rather
  than hand-writing serialization.

`wasmtime` adds ~15-20 MB to the binary due to the Cranelift JIT compiler. This
is a temporary cost: we will migrate to
[`wasmi`](https://github.com/wasmi-labs/wasmi) (an interpreter at ~1 MB) once it
gains component model support. The WIT interface stays the same; only the host
runtime changes. Track progress:

- [wasmi WIT discussion](https://github.com/wasmi-labs/wasmi/discussions/703)
- [wasmi WIT issue](https://github.com/wasmi-labs/wasmi/issues/657)

### Crate structure

Plugin configuration types (`SandboxConfig`, `CommandRule`, `NetworkSandbox`,
`FilesystemSandbox`, plugin entry deserialization) live in `jp_config` alongside
all other configuration types. This is where `[[plugins]]` entries are
deserialized.

The `jp_wasm` crate owns the Wasm runtime and all plugin execution:

- `wasmtime::Engine` and component caching
- Component loading, instantiation, and export probing
- `jp:host` import implementations (delegates to `std::process::Command`,
  `reqwest`, `std::fs` with sandbox enforcement)
- Secret scrubbing at the host boundary
- Inquiry-based permission prompts for unconfigured capabilities
- Capability-specific adapters (e.g. `WasmHandler` for attachments)

```txt
jp_cli
  ├── jp_config            <- SandboxConfig, plugin config types
  ├── jp_attachment
  │     └── jp_config
  └── jp_wasm              <- wasmtime runtime, sandbox enforcement,
        ├── jp_config           secret scrubbing, host imports
        └── wasmtime
```

Plugin loading is centralized in `jp_wasm`: it reads plugin config from
`jp_config`, loads all configured plugins, calls `plugin.name()`, discovers
capabilities, and hands off typed adapters to the relevant subsystems (e.g.
`WasmHandler` instances to `jp_attachment`). All plugins share the same `Engine`
instance and component cache.

## Security

Plugins run in an isolated sandbox. They have no direct access to the host
filesystem, network, or environment variables. All host interaction goes through
`jp:host` imports, and every call is checked against the plugin's sandbox
configuration before executing. Plugins do not use WASI filesystem, sockets, or
other WASI capabilities.

### Sandbox configuration

Each plugin has a sandbox config that controls its host-import capabilities.

```rust
// jp_config/src/plugin/sandbox.rs

/// Sandbox configuration for a Wasm plugin.
pub struct SandboxConfig {
    /// Per-command execution rules (governs `jp:host/process`).
    ///
    /// Keyed by program name. Only programs listed here can be
    /// executed. An empty map means no command execution.
    pub commands: HashMap<String, CommandRule>,

    /// Network access rules (governs `jp:host/http`).
    pub network: NetworkSandbox,

    /// Filesystem access rules (governs `jp:host/filesystem`).
    pub filesystem: FilesystemSandbox,
}

/// Rules for a single allowed command.
pub struct CommandRule {
    /// Allowed argument prefixes (sudo-style).
    ///
    /// Each entry is a sequence of values that must match the start
    /// of the actual arguments. `**` as the last element allows any
    /// remaining arguments.
    ///
    /// Examples:
    ///   ["log", "**"]      — allows `git log` with any flags
    ///   ["diff", "**"]     — allows `git diff` with any flags
    ///   ["status"]         — allows `git status` with no extra args
    ///
    /// If absent, any arguments are permitted.
    pub args: Option<Vec<Vec<String>>>,

    /// Environment variables forwarded to this command.
    ///
    /// The host reads values from its own environment and injects
    /// them into the subprocess. The plugin never sees the values.
    pub envs: Vec<String>,
}

pub struct NetworkSandbox {
    /// Allowed outbound URL prefixes.
    ///
    /// Empty means no network access. The prefix implicitly
    /// restricts protocol and domain:
    ///   ["https://api.example.com"] — HTTPS only, that host only
    pub allow: Vec<String>,

    /// Environment variables available for header substitution.
    ///
    /// Header values containing `${VAR}` are expanded with values
    /// from the host environment. Unknown or disallowed variables
    /// cause an error. The plugin never sees the resolved values.
    pub envs: Vec<String>,
}

pub struct FilesystemSandbox {
    /// Allowed path prefixes.
    ///
    /// The workspace root is always readable. Paths listed here are
    /// allowed in addition.
    pub allow: Vec<PathBuf>,

    /// Whether to allow write access to allowed paths.
    ///
    /// Default: false (read-only).
    pub writable: bool,
}
```

Defaults are deny-all:

| Capability | Default                   |
|------------|---------------------------|
| Commands   | Denied                    |
| Filesystem | Workspace root, read-only |
| Network    | Denied                    |

Examples:

```toml
# A plugin that runs git commands (read-only subcommands)
[[plugins]]
wasm = ".jp/plugins/git_stats.wasm"

[plugins.sandbox.commands.git]
args = [["log", "**"], ["diff", "**"], ["status"]]

# A plugin that calls a REST API with authentication
[[plugins]]
wasm = ".jp/plugins/jira.wasm"

[plugins.sandbox.network]
allow = ["https://jira.example.com"]
envs = ["JIRA_API_TOKEN"]

# A plugin that runs curl with an API token
[[plugins]]
wasm = ".jp/plugins/slack.wasm"

[plugins.sandbox.commands.curl]
envs = ["SLACK_TOKEN"]

[plugins.sandbox.network]
allow = ["https://slack.com/api"]

# A plugin that needs a system database outside the workspace
[[plugins]]
wasm = "~/.jp/plugins/bear.wasm"

[plugins.sandbox]
filesystem.allow = ["~/Library/Group Containers/9K33E3U3T4.net.shinyfrog.bear"]
```

### Environment variable isolation

Plugins cannot read the host's environment variables. There is no
`jp:host/environment` import. Env vars flow to subprocesses (via `process.run`
`envs`) and into HTTP headers (via `${VAR}` substitution) without the plugin
code ever seeing the resolved values. This keeps secrets out of plugin memory.

### Secret scrubbing

A plugin that runs a subprocess with forwarded env vars could observe secret
values in the command output — e.g. `curl -v` prints request headers including
`Authorization: Bearer <token>`. The same applies to HTTP responses if a server
echoes back authentication headers.

The host scrubs all data crossing the boundary from host back to plugin. Before
returning process output (`stdout`, `stderr`), HTTP response bodies, or file
contents to the plugin, the host scans for the resolved values of all env vars
that were forwarded (via `process.run` `envs`) or substituted (via `${VAR}` in
HTTP headers) and replaces them with `[REDACTED]`.

This follows GitHub Actions' approach to secret masking in workflow logs. The
same limitations apply:

- Short or common values (e.g. `true`, `8080`) may cause false positives in
  output.
- Encoded forms (base64, URL-encoding, hex) are not detected.
- Partial substring matches may garble unrelated output.

Scrubbing is defense-in-depth, not a guarantee. The primary defense is
per-command env var scoping — only commands explicitly configured to receive a
secret have it in their environment. Scrubbing catches accidental leakage and
naive exfiltration attempts.

### Inquiry-based permissions

When a plugin requests a capability not covered by the sandbox config, the host
prompts the user through JP's existing inquiry system rather than failing
immediately. This applies to all sandbox-governed operations: command execution,
env var forwarding, network access, and filesystem reads outside the workspace.

The prompt follows the same pattern as tool-call permission prompts:

- `y` — allow this specific request, this time only.
- `Y` — allow this specific request for the remainder of the current turn.

There is no "always allow" option at the prompt. JP never writes to
user-authored config files. To pre-authorize a capability permanently, the user
adds the appropriate entry to their sandbox config manually. The sandbox config
is the persisted form of these permissions — if it covers a request, no prompt
is shown.

This means a plugin can work without any sandbox config at all: the user
installs the Wasm binary, adds the `wasm` path to their config, and JP prompts
for each capability as the plugin requests it. The user can then optionally add
sandbox config entries to suppress future prompts.

### OS-level subprocess sandboxing (future)

The sandbox config controls which commands a plugin can launch and which env
vars they receive, but the subprocess itself runs with the user's full OS
permissions (network access, filesystem access beyond the allowed paths).
OS-level sandboxing — macOS `sandbox-exec`, Linux seccomp/landlock — could
restrict what subprocesses can do once launched. This is strictly additive: the
sandbox config remains the primary control surface, OS-level enforcement adds a
second layer.

## Drawbacks

- **Binary size.** `wasmtime` adds ~15-20 MB. This is significant for a CLI
  tool. The cost is shared across all plugin types and is temporary — we will
  switch to `wasmi` once it gains component model support.
- **Custom host imports are a maintenance commitment.** The `jp:host` interfaces
  are JP-specific. We own their evolution and backward compatibility. Each new
  import is a trust boundary.
- **Dynamic capability discovery loses compile-time type safety.** The host uses
  `wasmtime`'s dynamic API to probe exports, trading `bindgen!` compile-time
  checks for runtime assertions. The cost is boilerplate in `jp_wasm`, not a
  per-plugin cost.

## Alternatives

### No plugin system (status quo)

All extensions are Rust crates compiled into the binary. Works for the core team
but prevents third-party extensibility entirely.

### Dynamic libraries (`.so`/`.dylib`)

Native plugins are faster and simpler but lack sandboxing, are
platform-specific, and have ABI stability concerns. Wasm provides a universal
target, a standard interface (WIT), and memory safety by default.

### Use wasmi instead of wasmtime

`wasmi` is an interpreter-based Wasm runtime at ~1 MB. It does not support the
component model, so plugins would use a JSON-over-linear-memory ABI instead of
WIT. We chose `wasmtime` because WIT provides a substantially better author
experience: typed contracts, multi-language code generation via `wit-bindgen`,
and compile-time error detection.

We will switch to `wasmi` when it gains component model support. The migration
is transparent to plugin authors — the WIT interfaces stay the same, only the
host runtime changes. Track progress:

- [wasmi WIT discussion](https://github.com/wasmi-labs/wasmi/discussions/703)
- [wasmi WIT issue](https://github.com/wasmi-labs/wasmi/issues/657)

### Combinatorial worlds for capability discovery

Define one WIT world per combination of capability interfaces
(`attachment-plugin`, `tool-plugin`, `attachment-and-tool-plugin`, etc.). The
host tries each at instantiation time. This avoids dynamic probing but the
number of worlds grows as 2^N for N capability types — untenable past 3-4 types.

### Stub implementations in a single world

Define one world with all capability exports. Plugins implement no-ops for
capabilities they don't provide. Simple but forces all plugins to recompile
whenever JP adds a new capability type. Bad for ecosystem stability.

## Non-Goals

- **Hot-reloading.** Plugins are loaded at startup. Changing a Wasm binary
  requires restarting `jp`.
- **Cross-plugin communication.** Plugins cannot call each other.
- **Direct environment variable access.** Plugins cannot read the host's
  environment variables. This is a security invariant, not a limitation to be
  removed later.
- **WASI capabilities.** Plugins do not use WASI filesystem, sockets, or other
  capabilities. All host interaction goes through `jp:host` imports.

## Risks and Open Questions

1. **Host import surface area.** The `jp:host` interfaces start minimal. As
   plugins need more capabilities, the interface grows. Each addition is a new
   trust boundary. We should version these interfaces and resist adding
   capabilities without clear demand.

2. **Argument prefix matching edge cases.** The sudo-style prefix matching works
   well for subcommand tools (`git log`, `docker run`) but programs interpret
   arguments inconsistently: `--flag=value` vs `--flag value`, `-abc` vs `-a -b
   -c`, `--` as separator. Should the host normalize arguments before matching,
   or is literal prefix comparison sufficient for a first version?

3. **Plugin config in `jp_config`.** Plugin configuration types live in
   `jp_config` alongside all other config. As the plugin model grows (tool
   interface, LLM interface), the plugin config module may grow significantly.
   If it becomes unwieldy, a dedicated crate could be split out, but `jp_config`
   is the right home for now.

4. **Dynamic export naming.** The capability discovery mechanism relies on
   `wasmtime`'s `get_export_index` returning exports with predictable names
   (e.g. `jp:plugin/attachment`). The exact export name format for interface
   exports in the component model needs validation during prototyping.

## Future Work

- **HTTPS-based plugin loading.** Allow `wasm = "https://..."` in config. On
  first use, JP downloads the binary, prompts the user to accept (showing a
  hash), and caches it locally.
- **Plugin discovery/registry.** A curated list or package manager for community
  plugins.
- **Migration to wasmi.** When `wasmi` gains component model support, switch
  runtimes to reduce binary size by ~15 MB. The WIT interfaces stay the same;
  only the host crate changes.
- **OS-level subprocess sandboxing.** macOS `sandbox-exec`, Linux
  seccomp/landlock as an additive enforcement layer. See
  [Security](#os-level-subprocess-sandboxing-future).
- **Capability interfaces.** This RFD defines the plugin infrastructure.
  Individual capability interfaces are defined in their own RFDs:
  - Attachment handlers: [RFD 017]
  - Tool plugins: future RFD
  - LLM providers: future RFD

## Implementation Plan

### Phase 1: Plugin config and `jp_wasm` crate

- Add plugin configuration types to `jp_config`: `SandboxConfig`, `CommandRule`,
  `NetworkSandbox`, `FilesystemSandbox`, `[[plugins]]` entry deserialization.
- Create `crates/jp_wasm/` with `wasmtime` dependency.
- Define WIT for `jp:host/process`, `jp:host/http`, `jp:host/filesystem`.
- Implement host-side handlers for each import (delegates to
  `std::process::Command`, `reqwest`, `std::fs` with sandbox enforcement using
  config types from `jp_config`).
- Implement secret scrubbing in `jp_wasm`.
- Unit tests for sandbox enforcement and secret scrubbing.
- **Dependency:** None. Can merge independently.

### Phase 2: Plugin loading and capability discovery

- Define WIT for `jp:plugin` package (`plugin` interface, shared `types`).
- Implement plugin loader: read `plugins` config, compile Wasm components,
  instantiate, call `plugin.name()`, probe for capability exports.
- Implement inquiry-based permission prompts for unconfigured capabilities.
- Add top-level `plugins` config (array of strings or objects with `wasm` and
  `sandbox` fields).
- Integration test: load a minimal test plugin, verify name and capability
  discovery.
- **Dependency:** Phase 1.

### Phase 3: Attachment capability interface

- See [RFD 017] for attachment-specific implementation.
- **Dependency:** Phase 2.

### Phase 4: Documentation

- Plugin author guide: how to create a plugin, choose a world, implement
  interfaces, build for `wasm32-wasip2`.
- Sandbox configuration reference.
- Minimal example plugin (skeleton project).
- **Dependency:** Phase 2.

## References

- [RFD 015] — the native handler trait.
- [RFD 017] — first capability interface consumer.
- [Wasm Tools Architecture] — related tool plugin design.
- [WASI Preview 2 component model](https://component-model.bytecodealliance.org/)
- [WIT specification](https://component-model.bytecodealliance.org/design/wit.html)
- [wasmtime](https://github.com/bytecodealliance/wasmtime)
- [wasmtime `Instance::get_export_index`](https://docs.rs/wasmtime/latest/wasmtime/component/struct.Instance.html) —
  the API used for dynamic capability discovery.
- [wit-bindgen](https://github.com/bytecodealliance/wit-bindgen)
- [wasmi WIT discussion](https://github.com/wasmi-labs/wasmi/discussions/703) —
  track for planned migration to smaller runtime.
- [wasmi WIT issue](https://github.com/wasmi-labs/wasmi/issues/657)

[RFD 015]: 015-simplified-attachment-handler-trait.md
[RFD 017]: 017-wasm-attachment-handlers.md
[Wasm Tools Architecture]: ../architecture/wasm-tools.md
