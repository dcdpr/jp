# RFD 075: Tool Sandbox and Access Policy

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-01
- **Extends**: [RFD 016]
- **Requires**: [RFD 076]
- **Summary**: Enforces OS-level sandboxes (`sandbox-exec`, Landlock, and
  Windows job objects) and subprocess/environment rules on tool subprocesses.

## Summary

This RFD enforces OS-level sandboxing on subprocess-based tools using
`sandbox-exec` on macOS, Landlock on Linux, and restricted tokens with job
objects on Windows.
JP generates sandbox profiles from [RFD 076]'s `AccessPolicy`, which this RFD
extends with `CommandRule` subprocess restrictions and environment variable
isolation.
Unconfigured tools receive a default workspace-scoped sandbox with no network
access.

## Motivation

JP runs LLM-selected tools as subprocesses on the user's machine.
Today, the only safeguard is `RunMode::Ask`, a permission prompt before
execution.
Once approved, the subprocess runs with the user's full OS privileges: it can
read or modify any file, access the network, and spawn other processes.

Relying solely on a pre-execution prompt leaves four gaps:

1. **Tools can access files outside the workspace.** A `modify_file` tool given
   an absolute path like `/etc/hosts` or `~/.ssh/id_rsa` reads or writes it
   directly.
   `RunMode::Ask` displays the arguments, but a user skimming a long argument
   list can miss a dangerous path.

2. **Tools can access the network.** A shell-based tool can `curl` workspace
   contents to an external server; nothing prevents a tool subprocess from
   exfiltrating data.

3. **`RunMode` is all-or-nothing.** Permissions apply per invocation rather than
   per capability: a user can approve or deny a tool call, but cannot restrict
   it to reading `src/` or block its network access.

4. **`jp init` has to warn users that tools are unrestricted.** The setup wizard
   states that "externally supplied tools cannot be restricted" and "can
   potentially run any command on your system" because JP lacks a mechanism to
   constrain them.

[RFD 076] defines a typed `AccessPolicy` covering filesystem paths, network
URIs, and environment variables, along with a cooperative enforcement model
where tools check their own grants.
Cooperative checks produce clear error messages, but a buggy or malicious tool
can ignore them.

[RFD 016] defines per-capability sandboxing and inquiry-based permission prompts
for WASM plugins.
Because WASM has no ambient capabilities, that boundary holds for plugins, but
it does not cover subprocess-based tools, which make up the majority of tools.

This RFD adds mandatory, OS-level enforcement that a tool subprocess cannot
bypass.
It generates platform-specific sandbox profiles from [RFD 076]'s `AccessPolicy`,
extends `AccessPolicy` with subprocess spawn rules (`CommandRule`), and isolates
the subprocess environment.

## Design

### Relationship to RFD 076 (Tool Access Grants)

This RFD and [RFD 076] enforce the same access policy at different layers:

| Concern          | RFD 076                        | RFD 075                      |
| ---------------- | ------------------------------ | ---------------------------- |
| **Policy**       | Defines `AccessPolicy` types   | Consumes them                |
| **Enforcement**  | Tool self-checks (cooperative) | OS-level sandbox (mandatory) |
| **Failure mode** | Helpful error message          | Raw permission denied        |
| **Bypass**       | Tool can ignore                | OS enforces                  |
| **Config key**   | `conversation.tools.*.access`  | Same                         |

Both layers read from `conversation.tools.*.access`.
[RFD 076] checks the policy at the application level to report clear errors that
name the denied capability and list configured grants, while this RFD enforces
the same policy in the kernel so the tool process cannot bypass it.

### Extending `AccessPolicy` with `CommandRule`

[RFD 076] defines `AccessPolicy` with three resource types: filesystem (`fs`),
network (`net`), and environment variables (`env`).
This RFD adds a fourth: subprocess commands.

```rust
/// Extension to AccessPolicy for subprocess spawn restrictions.
///
/// When `None`, the tool may spawn any subprocess.
/// When `Some`, only listed programs are allowed.
pub commands: Option<HashMap<String, CommandRule>>,
```

```rust
pub struct CommandRule {
    /// Allowed argument prefixes.
    ///
    /// Each entry is a sequence of values that must match the start
    /// of the actual arguments. `**` as the last element allows any
    /// remaining arguments.
    ///
    /// If absent, any arguments are permitted.
    pub args: Option<Vec<Vec<String>>>,

    /// Environment variables forwarded to this command.
    ///
    /// Restricts which of the tool's allowed env vars are passed to
    /// this specific child process.
    pub envs: Vec<String>,
}
```

`CommandRule` is enforced by the OS (this RFD) and the WASM host ([RFD 016])
rather than cooperatively by the tool, because the tool is the subprocess being
constrained.
Placing `commands` on `AccessPolicy` keeps all capability rules under one
`access` configuration block:

```toml
[conversation.tools.cargo_check]
source = "local"
command = ".config/jp/tools/target/release/jp-tools cargo check"

[[conversation.tools.cargo_check.access.fs]]
path = "."
read = true
write = true

[conversation.tools.cargo_check.access.commands.cargo]
args = [["check", "**"]]

[conversation.tools.my_script]
source = "local"
command = "./scripts/deploy.sh"

[[conversation.tools.my_script.access.fs]]
path = "."
read = true
write = true

[[conversation.tools.my_script.access.net]]
host = "api.example.com"
scheme = "https"
allow = true

[[conversation.tools.my_script.access.env]]
name = "DEPLOY_TOKEN"
read = true
```

### Default policy

The OS sandbox applies to every subprocess tool.
Tools without an explicit `access` configuration receive a default policy:

| Capability  | Default                                |
| ----------- | -------------------------------------- |
| Filesystem  | Workspace root, read-write             |
| Network     | Denied                                 |
| Commands    | Unrestricted (no subprocess filtering) |
| Environment | Minimal set only (`PATH`, `HOME`,      |
|             | `USER`, `LANG`, locale)                |

Read-write workspace access avoids breaking existing tools while still confining
filesystem access to the workspace and blocking network access.
The defaults can be tightened once most tools declare explicit `access`
policies.

When `access` is omitted, JP materializes the default policy as an
`AccessPolicy` and passes `access: Some(default_policy)` in the tool's
`Context`.
Both the cooperative layer ([RFD 076]) and the OS sandbox enforce the same
policy.
`Context.access` is `None` only when OS-level sandboxing is unavailable (on an
unsupported platform or in fallback mode).

An explicit `access` configuration replaces the default policy.
JP generates the OS sandbox from the configured `AccessPolicy` using [RFD 076]'s
merge semantics: `fs`, `net`, and `env` each use `MergeableVec`, which appends
across config layers by default unless overridden with the `Merged` form
(replace, prepend, or dedup).

### Platform enforcement

#### macOS: `sandbox-exec`

On macOS, `sandbox-exec` applies a Scheme-based Sandbox Profile Language (SBPL)
profile to a process.
JP generates an SBPL profile from the tool's `AccessPolicy` and launches the
tool under it:

```txt
JP generates profile → sandbox-exec -p <profile> <command> <args>
```

The mapping from `AccessPolicy` to SBPL:

| AccessPolicy field            | SBPL rule                                          |
| ----------------------------- | -------------------------------------------------- |
| `FsRule { read: true }`       | `(allow file-read* (subpath "<resolved-path>"))`   |
| `FsRule { write: true }`      | `(allow file-write* (subpath "<resolved-path>"))`  |
| `NetRule` with port/scheme    | `(allow network-outbound (remote tcp "*:<port>"))` |
| `NetRule` without port/scheme | `(allow network-outbound (remote tcp))`            |
| No network rules              | `(deny network*)`                                  |

Because `sandbox-exec` does not distinguish write sub-operations, [RFD 076]'s
`create`, `update`, and `delete` flags all map to `file-write*`.
`sandbox-exec` operates at the TCP layer and filters network access by port
only; [RFD 076]'s `host` and `path_prefix` fields have no SBPL equivalent and
are enforced cooperatively.
When `port` is omitted, JP translates `scheme` to its default port (`80` for
`http`, `443` for `https`).

Example generated profile for a read-write workspace tool with no network:

```scheme
(version 1)
(deny default)
(allow process-exec)
(allow file-read*
  (subpath "/path/to/workspace"))
(allow file-write*
  (subpath "/path/to/workspace"))
(deny network*)
```

Limitations:

- **`sandbox-exec` is deprecated.** It remains functional on macOS 15, and Apple
  provides no equivalent public API for third-party applications.
  SBPL is undocumented (all references come from reverse engineering), and Apple
  has progressively restricted profile capabilities across releases.
  If a future macOS release removes `sandbox-exec`, JP falls back to [RFD 076]'s
  cooperative enforcement.
  A future RFD should evaluate macOS alternatives such as App Sandbox
  entitlements via XPC services or `posix_spawn` restrictions.
- **Path escaping and symlinks.** Profile generation must escape paths and
  resolve symlinks.
- **Coarse `process-exec` control.** `sandbox-exec` can only allow or deny
  `process-exec` globally, not per binary, so `commands` restrictions on macOS
  are enforced at the application level rather than by the OS.

#### Linux: Landlock

Landlock is a Linux security module (kernel 5.13+) that lets unprivileged
processes restrict their own path-level filesystem access and (since kernel 6.7)
TCP port access, mapping directly to [RFD 076]'s `FsRule` and `NetRule` types:

| AccessPolicy field        | Landlock flags                       |
| ------------------------- | ------------------------------------ |
| `FsRule { read: true }`   | `AccessFs::ReadFile                  |
| `FsRule { create: true }` | `AccessFs::MakeDir                   |
|                           | ...`                                 |
| `FsRule { update: true }` | `AccessFs::WriteFile                 |
|                           | ...`                                 |
| `FsRule { delete: true }` | `AccessFs::RemoveFile                |
|                           | AccessFs::RemoveDir`                 |
| `NetRule` (any allow)     | `AccessNet::ConnectTcp` + port rules |
|                           | (kernel 6.7+)                        |

Landlock preserves more of [RFD 076]'s filesystem granularity than
`sandbox-exec` (distinguishing read, create, update, and delete operations per
path) and restricts TCP connections by port on kernel 6.7+.
Like `sandbox-exec`, Landlock has no hostname or HTTP-path awareness, so `host`
and `path_prefix` are enforced cooperatively.

```rust
// Pseudocode for Landlock ruleset creation from AccessPolicy
let mut ruleset = Ruleset::new();

for rule in &policy.fs {
    let resolved = resolve_path(workspace_root, rule.path());
    let mut flags = AccessFs::empty();

    if rule.read()   { flags |= AccessFs::ReadFile | AccessFs::ReadDir; }
    if rule.create() { flags |= AccessFs::MakeDir | AccessFs::MakeReg; }
    if rule.update() { flags |= AccessFs::WriteFile; }
    if rule.delete() { flags |= AccessFs::RemoveFile | AccessFs::RemoveDir; }

    ruleset = ruleset.add_rule(PathBeneath::new(resolved, flags))?;
}

ruleset.restrict_self()?;
```

JP applies the Landlock ruleset in the child process after `fork()` and before
`exec()`, restricting the tool without affecting the JP process.
On kernels without Landlock (pre-5.13) or without `AccessNet` support (pre-6.7),
JP logs a warning and falls back to [RFD 076]'s cooperative enforcement.

Seccomp-bpf filters at the syscall level rather than the path level, where
blocking an unexpected syscall can crash the process.
Landlock is the primary Linux mechanism because its path model matches
`AccessPolicy`; seccomp-bpf may be added later as a supplementary layer.

#### Windows: best-effort enforcement

Windows provides two mechanisms that partially cover the sandbox model:

- **Restricted tokens** strip privileges from a process token before spawning.
- **Job objects** restrict child process creation, network access, and resource
  consumption.

Because Windows permissions use NTFS ACLs rather than path prefixes, enforcing
per-invocation path allowlists is expensive and fragile.
The initial Windows implementation enforces coarser boundaries than macOS and
Linux:

- **Network**: Coarse on/off restriction via job objects.
- **Child processes**: Subprocess spawn restrictions via job objects.
- **Filesystem**: No OS-level path restrictions; [RFD 076]'s cooperative policy
  is the sole filesystem control.

More capable Windows mechanisms such as AppContainers or Windows Sandbox are
left to a future RFD.

#### Unsupported platforms and fallback

On platforms where none of these mechanisms are available, JP logs a warning
once per session at startup and runs without OS-level sandboxing, leaving [RFD
076]'s cooperative policy and `RunMode` prompts in effect:

```text
Warning: OS-level tool sandboxing is not available on this platform.
Tools run with your full user permissions. Use `run = "ask"` for untrusted tools.
```

### Interaction with `RunMode`

`RunMode` and the sandbox operate at different stages and compose:

| Concern         | `RunMode`                      | Sandbox                                |
| --------------- | ------------------------------ | -------------------------------------- |
| **Question**    | "Should this tool run at all?" | "What can this tool do while running?" |
| **Granularity** | Per-invocation                 | Per-capability                         |
| **Timing**      | Before execution               | During execution                       |

With `run = "ask"` and no explicit `access` block, the user approves each
invocation and the tool runs under the default sandbox.
Most workspace tools can instead pair `run = "unattended"` with an explicit
policy so they run without prompts while the OS confines them to the workspace:

```toml
[conversation.tools.my_tool]
run = "unattended"

[[conversation.tools.my_tool.access.fs]]
path = "."
read = true
write = true
```

External or untrusted tools can combine `run = "ask"` with a restrictive access
policy.

### Sensitive path protection

Certain paths trigger a warning whenever a tool's access policy includes them,
because they hold credentials or secrets that rarely belong in a tool
subprocess:

- `~/.ssh/`: SSH keys
- `~/.gnupg/` and `~/.gpg/`: GPG keys
- `~/.aws/credentials`: AWS credentials
- `~/.config/gcloud/`: Google Cloud credentials
- `~/.kube/config`: Kubernetes credentials
- `~/.docker/config.json`: Docker credentials
- `**/.env` and `**/.env.*`: environment files

JP ships this list as a built-in default that cannot be reduced via
configuration.
Users can append additional patterns with `sensitive_paths`:

```toml
[conversation.tools.*.access]
sensitive_paths = ["~/.myapp/secrets", "**/.secret.*"]
```

`sensitive_paths` is a **warning list**, not a deny list.
When `access.fs` grants access to a sensitive path, the OS sandbox allows the
path, but JP prompts the user on first use with a warning naming the sensitive
path before running the tool.
The user's approval or denial is remembered for the session.
Even when a tool has both an explicit `access.fs` grant and `run =
"unattended"`, JP still shows the sensitive-path warning once per session.

For unconfigured tools, the default workspace sandbox automatically excludes
home-directory credentials outside the workspace.
Sensitive files inside the workspace (such as `.env` files) remain accessible in
the default sandbox, and trigger a warning if the tool's stderr output indicates
they were accessed.

### Environment variable isolation

By default, JP strips the subprocess environment down to a minimal set (`PATH`,
`HOME`, `USER`, `LANG`, and locale variables) so secrets in the shell
environment do not leak to tools.

Tools declare additional variables through [RFD 076]'s `EnvRule`:

```toml
[[conversation.tools.my_tool.access.env]]
name = "GITHUB_TOKEN"
read = true

[[conversation.tools.my_tool.access.env]]
name = "AWS_*"
read = true
```

JP forwards variables matching `EnvRule` entries with `read = true` alongside
the minimal set.
When a tool also defines `CommandRule` entries, each command's `envs` list
further restricts which of the tool's allowed variables reach that child
process.

### Profile generation and caching

JP generates sandbox profiles from `AccessPolicy` once at tool resolution time
(per `jp query` invocation rather than per tool call) and caches them for the
session.
Relative paths in `access.fs` are resolved against the workspace root at
generation time, and symlinks are canonicalized to prevent symlink-traversal
bypasses.

On macOS, JP writes the SBPL profile to a temporary file and passes it to
`sandbox-exec -f`.
On Linux, it builds the Landlock ruleset in memory; on Windows, it creates the
job object once and reuses it across calls.

## Drawbacks

- **Platform inconsistency.** Enforcement depth varies by OS: `sandbox-exec` is
  deprecated and undocumented, Landlock requires Linux 5.13+ (6.7+ for network
  rules), and Windows initially lacks per-path filesystem rules.

- **`sandbox-exec` deprecation risk.** Apple could remove `sandbox-exec` in a
  future macOS release without a public replacement, leaving a primary JP
  platform without OS-level sandboxing until an alternative lands.

- **Spawn overhead.** Wrapping a process in `sandbox-exec` on macOS or
  constructing a Landlock ruleset on Linux (~1ms) adds per-call startup cost,
  though it is small next to tool execution and LLM latency.

- **Opaque OS denials.** Default restrictions can break tools that read outside
  the workspace (such as system headers, package caches, or local databases).
  Because the kernel returns raw permission errors, users must infer that the
  sandbox denied the access and add the missing `access` rule.

- **Configuration burden.** While the default policy covers workspace-local
  tools, build tools that read toolchain directories or tools that require
  network access need explicit `access` rules.

- **Breaking change for environment inheritance.** Subprocesses previously
  inherited the full parent environment.
  Stripping all non-minimal variables requires explicit `EnvRule` entries for
  tools that depend on `CARGO_HOME`, `RUSTUP_HOME`, proxy variables, or other
  ambient settings.

## Alternatives

### No OS-level sandboxing (cooperative only)

Rely solely on [RFD 076]'s cooperative checks and a future `runtime = "vfs"`
mode that mediates host access over IPC.

Rejected because `runtime = "stdio"` is the default and will remain the most
common mode.
Until `runtime = "vfs"` exists, and for all `stdio` tools afterward, cooperative
checks alone cannot stop a buggy or malicious tool from ignoring the policy.

### Container-based sandboxing

Run each tool inside a container, namespace sandbox, or micro-VM.

Rejected as disproportionate to the threat model.
Containers add startup latency and setup complexity, and namespace isolation is
not natively available on macOS or Windows.
JP tools are typically short-lived, user-configured commands where the primary
risk is an unintended LLM argument rather than adversarial kernel exploitation.

### Seccomp-only on Linux

Use seccomp-bpf instead of Landlock on Linux.

Rejected as the primary mechanism because seccomp filters raw syscalls rather
than paths, and blocking an unexpected libc syscall crashes the process.
Landlock's path-based rules map directly to `AccessPolicy`.
Seccomp may be added later as a supplementary filter.

## Non-Goals

- **Sandboxing JP itself.** Only tool subprocesses are sandboxed; the JP process
  requires full filesystem, network, and process-table access.

- **Sandboxing MCP tools.** MCP tools execute on external servers managed by
  their operators.
  JP treats MCP responses like any other network response.

- **Restricting LLM API calls.** Provider connections are configured through
  provider settings and are not subject to tool sandboxes.

- **Fine-grained syscall filtering.** This RFD covers filesystem, network,
  environment, and subprocess execution rules.
  Blocking low-level syscalls such as `ptrace` or `mount` is out of scope.

- **Replacing `RunMode`.** `RunMode` gates whether a tool runs; the sandbox
  constrains what it can access while running.

- **Post-hoc detection of OS sandbox violations.** When the kernel denies an
  operation, the tool receives a raw permission error.
  JP cannot intercept kernel denials directly, and scraping stderr cannot
  reliably distinguish sandbox violations from ordinary permission errors.
  Users diagnose denials from the tool's output and update `access` accordingly;
  structured violation reporting is left to a future RFD.

- **Sanitizing terminal escape sequences in tool output.** Tool output and
  custom formatters can emit ANSI control sequences (such as OSC 52 clipboard
  writes, window-title changes, or cursor movement).
  Output sanitization belongs in the terminal render sink, which strips non-SGR
  escape sequences from tool output by default, rather than in the OS process
  sandbox.

## Risks and Open Questions

1. **`sandbox-exec` removal timeline.** Apple has deprecated `sandbox-exec`
   without a removal timeline or a public replacement API.
   If Apple removes it, macOS falls back to cooperative enforcement until an
   alternative sandbox mechanism is designed.

2. **Landlock kernel version adoption.** Filesystem rules require Linux 5.13+
   and network rules require 6.7+.
   While current desktop distributions generally ship 5.15+, older server and CI
   kernels may not; what minimum kernel version should JP target?

3. **Symlink and mount-point handling.** JP resolves symlinks when building the
   sandbox profile, but a symlink inside the workspace pointing outside it could
   still expose external targets or be created during execution.
   Should the sandbox also deny symlink traversal to out-of-scope paths?

4. **Convergence with RFD 016.** This RFD adds `CommandRule` to [RFD 076]'s
   `AccessPolicy`, whereas [RFD 016] defines its own `CommandRule` and
   `SandboxConfig` for WASM plugins.
   Once this RFD is accepted, [RFD 016] should adopt the shared `AccessPolicy`
   types; does the WASM sandbox need anything `AccessPolicy` does not express?

5. **Windows filesystem restrictions.** The initial Windows implementation
   enforces only coarse network and child-process limits.
   Is that sufficient for an initial release, or should Windows sandboxing wait
   for a path-capable mechanism?

6. **Environment variable discovery.** Stripping non-minimal environment
   variables by default breaks tools that rely on ambient settings such as
   toolchain paths, proxy variables, or editor preferences.
   Should JP log the stripped variable names when a tool exits with an error so
   users know which `EnvRule` entries to add?

## Implementation Plan

### Phase 1: `CommandRule` in `AccessPolicy`

- Add `commands: Option<HashMap<String, CommandRule>>` to `AccessPolicy` in
  `jp_tool`, extending [RFD 076]'s types.
- Add `commands` field support to the `access` config in `jp_config`.
- Add unit tests for command-argument prefix matching.
- **Dependency:** [RFD 076] Phase 1 (types in `jp_tool`).

### Phase 2: Environment variable isolation

- Call `env_clear()` when spawning tool subprocesses in `jp_llm` and forward
  only the minimal set (`PATH`, `HOME`, `USER`, `LANG`, locale) by default.
- Forward any additional variables matching `access.env` rules.
- Update JP's built-in tools and documentation for any newly required `EnvRule`
  entries.
- **Dependency:** Phase 1.

### Phase 3: macOS `sandbox-exec` integration

- Implement SBPL profile generation from `AccessPolicy`.
- Wrap tool subprocess spawning with `sandbox-exec` when available.
- Resolve paths and symlinks during profile generation.
- Implement sensitive-path warnings.
- Add integration tests that verify sandbox denials (macOS CI only).
- Feature-gate behind `#[cfg(target_os = "macos")]`.
- **Dependency:** Phase 2.

### Phase 4: Linux Landlock integration

- Implement Landlock ruleset generation from `AccessPolicy`.
- Apply the ruleset in the child process after `fork()` and before `exec()`.
- Detect kernel support and fall back with a warning when unavailable.
- Add integration tests (Linux CI only).
- Feature-gate behind `#[cfg(target_os = "linux")]`.
- **Dependency:** Phase 2.

### Phase 5: Windows best-effort enforcement

- Create job objects for network and child-process restrictions (without
  per-path filesystem rules).
- Assign spawned tool processes to the job object.
- Add integration tests (Windows CI only).
- Feature-gate behind `#[cfg(target_os = "windows")]`.
- **Dependency:** Phase 2.

## References

- [RFD 076]: Tool access grants (`AccessPolicy` types, cooperative enforcement,
  and filesystem, network, and environment rules).
- [RFD 016]: WASM plugin architecture, sandbox configuration, secret scrubbing,
  and inquiry-based permissions.
- [RFD 009]: Stateful tool protocol.
  OS sandboxing applies to both one-shot and stateful tool sessions.
- [Apple sandbox-exec man page]
- [Apple Sandbox SBPL Reference]
- [Landlock documentation]
- [landlock-rs crate]
- [Windows Job Objects]
- [Windows Restricted Tokens]
- [Chromium sandbox design (Windows)]

[Apple Sandbox SBPL Reference]: https://reverse.put.as/wp-content/uploads/2011/09/Apple-Sandbox-Guide-v1.0.pdf
[Apple sandbox-exec man page]: https://keith.github.io/xcode-man-pages/sandbox-exec.1.html
[Chromium sandbox design (Windows)]: https://chromium.googlesource.com/chromium/src/+/HEAD/docs/design/sandbox.md
[Landlock documentation]: https://docs.kernel.org/userspace-api/landlock.html
[RFD 009]: 009-stateful-tool-protocol.md
[RFD 016]: 016-wasm-plugin-architecture.md
[RFD 076]: 076-tool-access-grants.md
[Windows Job Objects]: https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects
[Windows Restricted Tokens]: https://learn.microsoft.com/en-us/windows/win32/secauthz/restricted-tokens
[landlock-rs crate]: https://crates.io/crates/landlock
