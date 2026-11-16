# RFD 016: Wasm Plugin Architecture

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-28
- **Required by**: [RFD 017]
- **Summary**: Defines a sandboxed Wasm component plugin system with WIT
  capability interfaces, dynamic export discovery, and per-plugin host
  permissions.

## Summary

This RFD defines a Wasm component plugin system for extending JP with attachment
handlers, tools, and other capabilities.
Each plugin exports a required `plugin` identity interface and optional
capability interfaces that the host discovers dynamically at load time via
`wasmtime`.
All host interaction runs through `jp:host` imports under a per-plugin sandbox
policy.

## Motivation

JP's extension points (attachment handlers, tools, and LLM providers) are
compiled into the binary.
Adding an attachment type or tool requires writing a Rust crate, wiring it into
the workspace, and recompiling, leaving users few ways to extend JP without
forking the repository.

We need a plugin system that:

1. Lets third parties extend JP without recompiling.
2. Runs untrusted code safely, without unrestricted access to the host
   filesystem, network, or environment.
3. Supports multiple capabilities in a single plugin (e.g. a Jira plugin that
   provides both an attachment handler and a tool).
4. Scales to many capability types without combinatorial complexity.
5. Works across platforms and languages.

The Wasm component model meets all five requirements: it is sandboxed by
default, cross-platform, multi-language (via `wit-bindgen`), and built around
composable typed interfaces.

## Design

### Interface model

A plugin is a Wasm component that exports interfaces from the `jp:plugin`
package.
Every plugin must export the `plugin` interface to identify itself.
Capability interfaces are optional; the host discovers which ones the component
exports and registers them.

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

Capability interfaces live in the same package, one per extension point:

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

JP publishes these interfaces, and plugin authors compose a world from the
interfaces they implement:

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

On the guest side, `wit-bindgen` generates typed bindings for that world, so the
plugin implements only the interfaces it exports, without stubs or unused code.

JP also provides optional convenience worlds for common cases, though plugin
authors can always define their own:

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

### Capability discovery

WIT does not support optional exports: a component must implement every export
in its world.
Because the host does not need to know the guest's world ahead of time, it uses
`wasmtime`'s runtime API to probe which interfaces a component exports.

At load time, the host:

1. Instantiates the component and links all `jp:host/*` imports via the
   `Linker`.
2. Calls `instance.get_export_index(None, "jp:plugin/plugin")`, returning an
   error if the export is absent.
3. Calls `plugin.name()` to identify the plugin.
4. Probes for each known capability interface and registers the ones present:
   - `instance.get_export_index(None, "jp:plugin/attachment")` registers an
     attachment handler.
   - `instance.get_export_index(None, "jp:plugin/tool")` registers a tool
     provider.

Adding a new capability requires only a new WIT interface and one host-side
probe call.
Existing plugins do not export the new interface, so the host skips it without
requiring them to recompile.

As a trade-off, the host uses `wasmtime`'s dynamic API (`get_func`,
`get_typed_func`) instead of static `bindgen!` bindings, trading compile-time
checks for runtime type assertions and extra host boilerplate.
That cost is confined to `jp_wasm` and does not grow with the number of plugins
or capabilities.

### Host imports

Plugins do not use WASI capabilities directly.
All host interaction goes through the `jp:host` package, where the host checks
every call against the plugin's sandbox configuration before executing it.

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

Guests import only what they need: a pure data-transformation plugin imports
nothing, a plugin that runs `git` imports `jp:host/process`, and a plugin that
calls an HTTP API imports `jp:host/http`.

The interfaces are intentionally narrow.
`process.run` runs a command in a clean environment and returns its output
rather than exposing raw `fork`/`exec`, and `http.get` performs a single GET
request with optional headers rather than providing a full HTTP client.
Additional operations (e.g. `http.post`, `process.run_streaming`) can be added
as needed.

### Plugin configuration

Plugins are configured in a top-level `plugins` array.
When default permissions suffice, each entry can be a Wasm path string:

```toml
plugins = ["simple_plugin.wasm", "another_plugin.wasm"]
```

Plugins that need custom sandbox rules use the array-of-tables form:

```toml
[[plugins]]
wasm = ".jp/plugins/jira.wasm"

[plugins.sandbox.network]
allow = ["https://jira.example.com"]
envs = ["JIRA_API_TOKEN"]

[[plugins]]
wasm = "~/.jp/plugins/notion.wasm"
```

The `wasm` key sets the component path, and the plugin format is inferred from
the key (currently always Wasm, with room for other formats later).
Config entries have no name field because each plugin identifies itself through
`plugin.name()`; if two plugins return the same name, the host fails at startup
with an error naming both.
Components are compiled on first use and cached for the lifetime of the process.

### Wasm runtime

The host runs plugins on [`wasmtime`] with the WASI Preview 2 component model,
and plugin authors generate guest bindings with [`wit-bindgen`].

Compared to a custom ABI, WIT gives plugin authors:

- **Typed contracts.** WIT definitions serve as both interface documentation and
  code-generation source, catching type mismatches at compile time.
- **Multi-language bindings.** `wit-bindgen` generates idiomatic glue code and
  serialization for Rust, Go, Python, C, and other languages.

`wasmtime` adds 15 to 20 MB to the binary because it bundles the Cranelift JIT
compiler.
Once the [`wasmi`] interpreter (~1 MB) supports the component model ([wasmi WIT
discussion], [wasmi WIT issue]), the host can switch runtimes without changing
the WIT interfaces.

### Crate structure

Plugin configuration types (`SandboxConfig`, `CommandRule`, `NetworkSandbox`,
`FilesystemSandbox`) and `[[plugins]]` deserialization live in `jp_config`
alongside the rest of JP's configuration.

The `jp_wasm` crate owns the Wasm runtime and plugin execution:

- `wasmtime::Engine` and component caching
- Component loading, instantiation, and export probing
- `jp:host` import implementations (delegating to `std::process::Command`,
  `reqwest`, and `std::fs` with sandbox enforcement)
- Secret scrubbing at the host boundary
- Inquiry-based permission prompts for unconfigured capabilities
- Capability-specific adapters (e.g. `WasmHandler` for attachments)

<!-- end list -->

```txt
jp_cli
  ├── jp_config            <- SandboxConfig, plugin config types
  ├── jp_attachment
  │     └── jp_config
  └── jp_wasm              <- wasmtime runtime, sandbox enforcement,
        ├── jp_config           secret scrubbing, host imports
        └── wasmtime
```

At startup, `jp_wasm` reads the plugin configuration from `jp_config`, loads
each plugin into a shared `Engine` and component cache, calls `plugin.name()`,
probes its capability exports, and passes typed adapters such as `WasmHandler`
to the consuming crates (e.g. `jp_attachment`).

## Security

Plugins run in an isolated sandbox with no WASI filesystem, socket, or
environment capabilities.
They cannot access the host filesystem, network, or environment variables
directly; every host operation goes through a `jp:host` import and is checked
against the plugin's sandbox configuration.

### Sandbox configuration

`SandboxConfig` defines the host permissions for a single plugin:

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

By default, only read access to the workspace root is allowed:

| Capability | Default                   |
| ---------- | ------------------------- |
| Commands   | Denied                    |
| Filesystem | Workspace root, read-only |
| Network    | Denied                    |

Example configurations:

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

There is no `jp:host/environment` import, so plugins cannot read host
environment variables.
Instead, the host forwards allowed variables directly to subprocesses (via
`envs` in `process.run`) and substitutes `${VAR}` placeholders in HTTP headers,
keeping secret values out of plugin memory.

### Secret scrubbing

Even without direct access to environment variables, a plugin could observe
secrets echoed in command output (e.g. `curl -v` printing an `Authorization:
Bearer <token>` header) or in HTTP responses.

To prevent this, the host scrubs all data returning to a plugin.
Before passing back process output (`stdout`, `stderr`), HTTP response bodies,
or file contents, the host replaces the resolved value of every forwarded or
header-substituted environment variable with `[REDACTED]`.

This matches the secret masking used in GitHub Actions workflow logs and shares
its limitations:

- Short or common values (e.g. `true`, `8080`) can cause false positives.
- Encoded values (base64, URL-encoding, hex) are not detected.
- Substring matches can redact parts of unrelated output.

Scrubbing is defense in depth, not a primary boundary.
The main defense is per-command environment scoping, which exposes a secret only
to the commands configured to receive it; scrubbing catches accidental leaks and
naive exfiltration.

### Inquiry-based permissions

When a plugin requests an operation not covered by its sandbox configuration
(command execution, environment variable forwarding, network access, or
filesystem reads outside the workspace), the host prompts the user through JP's
inquiry system instead of failing immediately.

The prompt uses the same options as tool-call permission prompts:

- `y`: allow this request once.
- `Y`: allow this request for the rest of the current turn.

There is no "always allow" option because JP never modifies user-authored config
files.
A plugin can run with only its `wasm` path configured, prompting on each
restricted operation until the user adds the corresponding rules to
`plugins.sandbox` to pre-authorize them.

### OS-level subprocess sandboxing (future)

The sandbox configuration controls which commands a plugin can launch and which
environment variables they receive, but a launched subprocess still runs with
the user's full OS permissions (including network and filesystem access outside
the plugin's allowed paths).
OS-level sandboxing (such as macOS `sandbox-exec` or Linux seccomp and Landlock)
can restrict what the subprocess itself can do once started, adding a second
enforcement layer beneath the sandbox configuration.

## Drawbacks

- **Binary size.** `wasmtime` adds 15 to 20 MB to the CLI binary, though this
  cost is shared across all plugin types and will drop once `wasmi` supports the
  component model.
- **Maintenance of custom host imports.** The `jp:host` interfaces are
  JP-specific, so JP owns their evolution, backward compatibility, and security
  review as each new import introduces a trust boundary.
- **Runtime type checks on the host.** Dynamic export probing uses `wasmtime`'s
  dynamic API instead of static `bindgen!` bindings, replacing compile-time
  checks with runtime assertions and extra boilerplate in `jp_wasm`.

## Alternatives

### No plugin system (status quo)

Keep all extensions as Rust crates compiled into the binary.
This works for built-in features but prevents third parties from extending JP
without forking.

### Dynamic libraries (`.so`/`.dylib`)

Load native shared libraries at runtime.
Native plugins are faster and simpler to host, but they are platform-specific,
lack a stable Rust ABI, and run unsandboxed.
Wasm provides a portable target, typed WIT interfaces, and memory isolation by
default.

### Use `wasmi` instead of `wasmtime`

`wasmi` is a ~1 MB interpreter, but because it does not yet support the
component model, using it now would require a custom JSON-over-linear-memory ABI
instead of WIT.
We chose `wasmtime` so plugin authors get typed WIT contracts and `wit-bindgen`
code generation from day one, with a transparent host-side migration to `wasmi`
once component model support lands ([wasmi WIT discussion], [wasmi WIT issue]).

### Combinatorial worlds for capability discovery

Define one WIT world per combination of capability interfaces
(`attachment-plugin`, `tool-plugin`, `attachment-and-tool-plugin`, and so on)
and try each at instantiation time.
This avoids dynamic export probing, but the number of worlds grows as 2^N for N
capability types, which becomes unmanageable past three or four capabilities.

### Stub implementations in a single world

Define a single world exporting every capability interface and require plugins
to implement no-op stubs for capabilities they do not provide.
This keeps host bindings static, but adding any new capability interface breaks
backward compatibility and forces every existing plugin to recompile.

## Non-Goals

- **Hot-reloading.** Plugins load once at startup; replacing a Wasm binary
  requires restarting `jp`.
- **Cross-plugin communication.** Plugins cannot call one another.
- **Direct environment variable access.** Plugins cannot read host environment
  variables; this is a permanent security invariant.
- **WASI capabilities.** Plugins do not use WASI filesystem, socket, or
  environment APIs; all host access goes through `jp:host` imports.

## Risks and Open Questions

1. **Host import surface area.** Each addition to `jp:host` widens the trust
   boundary.
   These interfaces must be versioned and expanded only when concrete plugin use
   cases require it.

2. **Argument prefix matching edge cases.** Sudo-style prefix matching fits
   subcommand tools (`git log`, `docker run`), but programs parse flags
   inconsistently (`--flag=value` vs `--flag value`, combined short flags like
   `-abc` vs `-a -b -c`, and `--` separators).
   We need to decide whether literal prefix matching is sufficient initially or
   whether the host should normalize arguments first.

3. **Plugin config in `jp_config`.** As tool and LLM plugin interfaces are
   added, the plugin configuration types in `jp_config` may grow large enough to
   warrant a dedicated crate, though `jp_config` is the right place to start.

4. **Dynamic export naming.** Capability discovery relies on `wasmtime`'s
   `get_export_index` matching interface names such as `jp:plugin/attachment`.
   The exact export naming convention for component model interfaces needs to be
   verified during prototyping.

## Future Work

- **HTTPS-based plugin loading.** Support `wasm = "https://..."` in config so JP
  downloads the binary on first use, prompts the user to verify its hash, and
  caches it locally.
- **Plugin discovery/registry.** Provide a curated directory or package manager
  for community plugins.
- **Migration to `wasmi`.** Switch the host runtime to `wasmi` once it supports
  the component model, reducing binary size by ~15 MB without changing the WIT
  interfaces.
- **OS-level subprocess sandboxing.** Add macOS `sandbox-exec` and Linux
  seccomp/Landlock enforcement for plugin subprocesses.
  See [Security](#os-level-subprocess-sandboxing-future).
- **Capability interfaces.** Individual capability interfaces are specified in
  their own RFDs:
  - Attachment handlers: [RFD 017]
  - Tool plugins: future RFD
  - LLM providers: future RFD

## Implementation Plan

### Phase 1: Plugin config and `jp_wasm` crate

- Add plugin configuration types (`SandboxConfig`, `CommandRule`,
  `NetworkSandbox`, `FilesystemSandbox`) and `[[plugins]]` deserialization to
  `jp_config`.
- Create `crates/jp_wasm/` with the `wasmtime` dependency.
- Define WIT interfaces for `jp:host/process`, `jp:host/http`, and
  `jp:host/filesystem`.
- Implement host-side handlers for each import (`std::process::Command`,
  `reqwest`, and `std::fs`) with sandbox enforcement.
- Implement secret scrubbing in `jp_wasm` and add unit tests for sandbox
  enforcement and scrubbing.
- **Dependency:** None; can merge independently.

### Phase 2: Plugin loading and capability discovery

- Define the `jp:plugin` WIT package (`plugin` interface and shared `types`).
- Add the top-level `plugins` config key (accepting path strings or tables with
  `wasm` and `sandbox` fields).
- Implement the plugin loader to compile and instantiate configured components,
  call `plugin.name()`, and probe for capability exports.
- Implement inquiry-based permission prompts for unconfigured capabilities.
- Add integration tests that load a minimal plugin and verify name and
  capability discovery.
- **Dependency:** Phase 1.

### Phase 3: Attachment capability interface

- See [RFD 017] for the attachment capability implementation.
- **Dependency:** Phase 2.

### Phase 4: Documentation

- Write the plugin author guide covering world selection, interface
  implementation, and building for `wasm32-wasip2`.
- Document the sandbox configuration reference.
- Provide a minimal example plugin project.
- **Dependency:** Phase 2.

## References

- [RFD 015]: the native attachment handler trait.
- [RFD 017]: the first capability interface built on this architecture.
- [Wasm Tools Architecture]: related tool plugin design.
- [WASI Preview 2 component model]
- [WIT specification]
- [wasmtime][`wasmtime`]
- [wasmtime `Instance::get_export_index`]: runtime API used for dynamic
  capability discovery.
- [wit-bindgen][`wit-bindgen`]
- [wasmi WIT discussion] and [wasmi WIT issue]: tracking for component model
  support in `wasmi`.

[RFD 015]: 015-simplified-attachment-handler-trait.md
[RFD 017]: 017-wasm-attachment-handlers.md
[WASI Preview 2 component model]: https://component-model.bytecodealliance.org/
[WIT specification]: https://component-model.bytecodealliance.org/design/wit.html
[Wasm Tools Architecture]: ../wiki/00dxyb9-wasm-tools-architecture.md
[`wasmi`]: https://github.com/wasmi-labs/wasmi
[`wasmtime`]: https://github.com/bytecodealliance/wasmtime
[`wit-bindgen`]: https://github.com/bytecodealliance/wit-bindgen
[wasmi WIT discussion]: https://github.com/wasmi-labs/wasmi/discussions/703
[wasmi WIT issue]: https://github.com/wasmi-labs/wasmi/issues/657
[wasmtime `Instance::get_export_index`]: https://docs.rs/wasmtime/latest/wasmtime/component/struct.Instance.html
