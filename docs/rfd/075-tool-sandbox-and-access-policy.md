# RFD 075: Tool Sandbox and Access Policy

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-01
- **Extends**: [RFD 016]
- **Requires**: [RFD 076]

## Summary

This RFD introduces OS-level sandboxing for subprocess-based tools.
Sandbox profiles are generated from the access policy defined in [RFD 076] and
applied per tool invocation using platform-native mechanisms: `sandbox-exec` on
macOS, Landlock on Linux, and restricted tokens with job objects on Windows.
This RFD also extends [RFD 076]'s `AccessPolicy` with `CommandRule` for
subprocess spawn restrictions, and defines the environment variable isolation
model for tool subprocesses.
Unconfigured tools receive a default sandbox (workspace read-write, no network,
minimal environment).

## Motivation

JP runs LLM-selected tools as subprocesses on the user's machine.
Today, the only safeguard is `RunMode::Ask` — a permission prompt before
execution.
Once the user approves, the subprocess runs with the user's full OS privileges:
it can read any file, access the network, spawn other processes, and modify
anything the user can.

This is a structural problem, not a configuration oversight:

1. **Tools can access files outside the workspace.** A `modify_file` tool
   receiving an absolute path argument like `/etc/hosts` or `~/.ssh/id_rsa` will
   happily read or write it.
   The `RunMode::Ask` prompt shows the arguments, but a user skimming a long
   argument list can miss a dangerous path.

2. **Tools can access the network.** A shell-based tool could `curl` data to an
   external server.
   Nothing prevents exfiltration of workspace contents through a tool
   subprocess.

3. **`RunMode` is all-or-nothing.** The current permission model is "run this
   tool: yes or no."
   There is no way to say "run this tool, but only let it read files in `src/`"
   or "run this tool, but block network access."
   The granularity is per-invocation, not per-capability.

4. **The init wizard warns about this explicitly.** The `jp init` command tells
   users that "externally supplied tools cannot be restricted" and that tools
   "can potentially run any command on your system."
   This is honest, but the right answer is to fix the problem, not document it.

[RFD 076] defines a typed access policy (`AccessPolicy`) that declares what
resources a tool can access — filesystem paths, network URIs, environment
variables — and a cooperative enforcement model where tools self-check their
grants.
That model addresses the policy surface and provides good error messages, but it
is cooperative: a buggy or malicious tool can ignore the policy.

[RFD 016] defines a sandbox model for WASM plugins with per-capability
configuration and inquiry-based permission prompts.
The WASM sandbox is architecturally sound (WASM has no ambient capabilities),
but it only applies to WASM plugins.
Subprocess-based tools — which are the vast majority of tools today and will
remain common — have no equivalent protection.

This RFD adds the mandatory enforcement layer: OS-level sandboxing that the tool
subprocess cannot bypass.
It consumes [RFD 076]'s `AccessPolicy` types to generate platform-specific
sandbox profiles, extends the policy with subprocess spawn restrictions
(`CommandRule`), and defines environment variable isolation for tool
subprocesses.

## Design

### Relationship to RFD 076 (Tool Access Grants)

This RFD and [RFD 076] address the same access policy at different enforcement
layers:

| Concern          | RFD 076                        | RFD 075                      |
| ---------------- | ------------------------------ | ---------------------------- |
| **Policy**       | Defines `AccessPolicy` types   | Consumes them                |
| **Enforcement**  | Tool self-checks (cooperative) | OS-level sandbox (mandatory) |
| **Failure mode** | Helpful error message          | Raw permission denied        |
| **Bypass**       | Tool can ignore                | OS enforces                  |
| **Config key**   | `conversation.tools.*.access`  | Same                         |

The access policy is configured once via `conversation.tools.*.access`.
[RFD 076]'s cooperative layer checks it at the application level with clear
error messages.
This RFD's OS layer enforces it at the kernel level as a hard boundary.

The two layers complement each other: RFD 076 provides a good user experience
(clear errors naming denied capabilities and listing configured grants).
This RFD provides a security boundary (the OS prevents access regardless of what
the tool code does).

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

`CommandRule` cannot be self-enforced by tools (the tool IS the subprocess), so
it exists for OS-level enforcement (this RFD) and WASM host enforcement ([RFD
016]).
It is included in `AccessPolicy` to keep the policy surface unified — one
config block, multiple enforcement layers.

Example configuration using [RFD 076]'s `access` key with the `commands`
extension:

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

The OS sandbox always applies to subprocess tools, even when no `access` config
is present.
Tools without explicit access configuration receive these defaults:

| Capability  | Default                                |
| ----------- | -------------------------------------- |
| Filesystem  | Workspace root, read-write             |
| Network     | Denied                                 |
| Commands    | Unrestricted (no subprocess filtering) |
| Environment | Minimal set only (`PATH`, `HOME`,      |
|             | `USER`, `LANG`, locale)                |

These defaults are deliberately permissive for filesystem access (read-write) to
avoid breaking existing tools.
The primary security value of the defaults is containing tools to the workspace
and blocking network access.
As the ecosystem matures and tools gain explicit `access` configuration, the
defaults can be tightened.

When no explicit `access` config is present, JP materializes the default policy
as an `AccessPolicy` and includes it in the tool's `Context`.
The tool sees `access: Some(default_policy)` — never `None` — so it can
self-check against the same restrictions the OS enforces.
This ensures the cooperative layer ([RFD 076]) and the OS layer always operate
on the same policy.
`Context.access` is `None` only when OS-level sandboxing is unavailable
(unsupported platform, fallback mode).

When explicit `access` config is present, it replaces the defaults entirely.
The OS sandbox is generated from the configured `AccessPolicy`, and [RFD 076]'s
merge semantics apply: `fs`, `net`, and `env` are each a `MergeableVec`, which
defaults to append across config layers.
Users opt into replace, prepend, or dedup per field via the explicit `Merged`
form (see [RFD 076]'s Cross-layer merging section).

### Platform enforcement

#### macOS: `sandbox-exec`

macOS provides `sandbox-exec`, which applies a Scheme-based sandbox profile to a
process.
JP generates a profile from the tool's `AccessPolicy` and launches the tool
process under it.

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

RFD 076's fine-grained `create`/`update`/`delete` distinctions map to a single
`file-write*` on macOS — sandbox-exec does not distinguish write
sub-operations.
RFD 076's `host` and `path_prefix` fields have no direct SBPL equivalent:
`sandbox-exec` operates at the TCP layer and cannot filter by hostname or HTTP
path.
Host and path filtering is enforced cooperatively ([RFD 076]); OS-level
enforcement on macOS is port-based only.
The `scheme` field is translated to its default port (`80` for `http`, `443` for
`https`) when no explicit `port` is specified.

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

- **`sandbox-exec` is deprecated by Apple** but still functional as of macOS 15.
  There is no replacement API with equivalent functionality for third-party
  applications.
  The profile language (SBPL) is undocumented — all available references are
  from reverse engineering.
  Apple has progressively restricted sandbox profile capabilities in recent
  releases.
  If Apple removes `sandbox-exec` in a future release, JP falls back to no
  OS-level enforcement on macOS (the cooperative policy from [RFD 076] remains
  active).
  A future RFD should investigate alternative macOS sandboxing (App Sandbox
  entitlements via XPC services, `posix_spawn` with manual restriction) before
  this becomes urgent.
- Profile generation must handle path escaping and symlink resolution.
- `sandbox-exec` cannot restrict which programs the subprocess spawns in a
  fine-grained way — it can deny `process-exec` entirely or allow it entirely.
  `commands` restrictions are enforced at the application level, not the OS
  level on macOS.

#### Linux: Landlock

Landlock is a Linux security module (available since kernel 5.13) that allows
unprivileged processes to restrict their own filesystem access.
Unlike seccomp (which filters syscalls), Landlock operates at the filesystem
level — it restricts which paths a process can access, which maps directly to
[RFD 076]'s `FsRule` model.

The mapping from `AccessPolicy` to Landlock:

| AccessPolicy field        | Landlock flags                       |
| ------------------------- | ------------------------------------ |
| `FsRule { read: true }`   | \`AccessFs::ReadFile                 |
| `FsRule { create: true }` | \`AccessFs::MakeDir                  |
|                           | ...\`                                |
| `FsRule { update: true }` | \`AccessFs::WriteFile                |
|                           | ...\`                                |
| `FsRule { delete: true }` | \`AccessFs::RemoveFile               |
|                           | AccessFs::RemoveDir\`                |
| `NetRule` (any allow)     | `AccessNet::ConnectTcp` + port rules |
|                           | (kernel 6.7+)                        |

Landlock preserves more of RFD 076's granularity than sandbox-exec for
filesystem access — it can distinguish read-only from write access per path,
and with kernel 6.7+, it can restrict TCP connections by port.
As with `sandbox-exec`, Landlock has no hostname or HTTP-path awareness; RFD
076's `host` and `path_prefix` fields are enforced cooperatively only.

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

JP uses Landlock by setting up the ruleset in the child process after `fork()`
but before `exec()`.
This restricts the tool process without affecting JP itself.

On older kernels without Landlock, or without `AccessNet` support (pre-6.7),
enforcement falls back gracefully: a warning is logged and the cooperative
policy ([RFD 076]) remains the only protection.

Seccomp-bpf is an alternative that filters at the syscall level.
It is more powerful but also more complex and fragile — blocking the wrong
syscall can crash the process in unpredictable ways.
Landlock is preferred because its filesystem-level model maps directly to
`AccessPolicy`.
Seccomp may be added as a supplementary layer in the future.

#### Windows: best-effort enforcement

Windows provides two mechanisms that partially address the sandbox requirements:

**Restricted tokens** strip privileges from a process token before spawning.
**Job objects** restrict the subprocess's ability to spawn child processes,
access the network, and consume resources.

Windows filesystem permissions are ACL-based, not path-prefix-based.
Expressing "allow only these paths" without configuring NTFS ACLs on a
per-invocation basis is expensive and fragile.
The initial Windows implementation provides coarser enforcement than
macOS/Linux:

- **Network**: Job objects can restrict network access (coarse on/off).
- **Child processes**: Job objects can restrict subprocess spawning.
- **Filesystem**: No per-path restriction in the initial implementation.
  The cooperative policy ([RFD 076]) is the primary filesystem control on
  Windows.

If a more capable Windows sandboxing approach becomes feasible (e.g., using
Windows Sandbox or AppContainers), a future RFD will address it.
The current design provides what value it can without overinvesting in a
platform where the OS mechanisms don't map cleanly to the policy model.

#### Unsupported platforms and fallback

On platforms where none of the above mechanisms are available, JP logs a warning
at startup and operates without OS-level enforcement.
The cooperative policy from [RFD 076] and `RunMode` permission prompts remain
active.

The warning is shown once per session, not per tool invocation:

```text
Warning: OS-level tool sandboxing is not available on this platform.
Tools run with your full user permissions. Use `run = "ask"` for untrusted tools.
```

### Interaction with `RunMode`

The sandbox system and `RunMode` serve different purposes and stack:

| Concern         | `RunMode`                      | Sandbox                                |
| --------------- | ------------------------------ | -------------------------------------- |
| **Question**    | "Should this tool run at all?" | "What can this tool do while running?" |
| **Granularity** | Per-invocation                 | Per-capability                         |
| **Timing**      | Before execution               | During execution                       |

A tool with `run = "unattended"` and a restrictive access policy is safe: it
runs without permission prompts but can only access what the policy allows.
A tool with `run = "ask"` and no access policy is the current behavior: the user
approves each invocation but the tool has the default sandbox restrictions.

The recommended configuration for most tools:

```toml
[conversation.tools.my_tool]
run = "unattended"

[[conversation.tools.my_tool.access.fs]]
path = "."
read = true
write = true
```

This is "trust but verify" — the tool runs without interruption, but the OS
prevents it from escaping the workspace.
For external or untrusted tools, combine `run = "ask"` with a restrictive access
policy for defense in depth.

### Sensitive path protection

Certain paths trigger a strong warning when a tool's access policy includes
them, regardless of how the policy is configured.
These paths represent high-value secrets that should rarely be exposed to tool
subprocesses:

- `~/.ssh/` — SSH keys
- `~/.gnupg/` or `~/.gpg/` — GPG keys
- `~/.aws/credentials` — AWS credentials
- `~/.config/gcloud/` — Google Cloud credentials
- `~/.kube/config` — Kubernetes credentials
- `~/.docker/config.json` — Docker credentials
- Files matching `**/.env` and `**/.env.*` patterns

JP ships this list as a built-in default, updated with each release.
Users can extend the list with additional paths:

```toml
[conversation.tools.*.access]
sensitive_paths = ["~/.myapp/secrets", "**/.secret.*"]
```

User-specified paths are added to (not replace) the built-in list.
The built-in list cannot be reduced via configuration.

Sensitive paths are a **warning list**, not a deny list.
When a tool's `access.fs` rules grant access to a sensitive path, the OS sandbox
honors the grant — the tool gets access.
But JP surfaces an inquiry prompt with a clear warning explaining what the tool
will be able to access before allowing it.
The user can approve or deny.
This applies on first use, not on every invocation — the user's response is
remembered for the session.
To permanently grant a tool access to a sensitive path without prompts, the user
adds the `access.fs` entry and configures `run = "unattended"` for that tool —
the warning is shown once per session regardless.

Tools without explicit `access` config receive the default sandbox (workspace
read-write), which excludes sensitive paths outside the workspace by definition.
Sensitive paths inside the workspace (e.g., `.env` files) are included in the
default sandbox but trigger the warning if the tool's stderr output indicates it
accessed them.

### Environment variable isolation

By default, tool subprocesses receive a minimal environment: `PATH`, `HOME`,
`USER`, `LANG`, and locale variables.
All other environment variables are stripped.

Tools that need specific environment variables declare them via [RFD 076]'s
`EnvRule`:

```toml
[[conversation.tools.my_tool.access.env]]
name = "GITHUB_TOKEN"
read = true

[[conversation.tools.my_tool.access.env]]
name = "AWS_*"
read = true
```

Only variables matching `EnvRule` entries with `read = true` are forwarded to
the subprocess, in addition to the minimal set.
This prevents accidental leakage of secrets through the subprocess environment.

For tools with `CommandRule` entries, per-command `envs` further restrict which
of the tool's allowed env vars are forwarded to specific child processes.

### Profile generation and caching

Sandbox profiles are generated from `AccessPolicy` at tool resolution time (once
per `jp query` invocation, not per tool call).
The generated profile is cached for the duration of the session.

On macOS, the profile is written to a temporary file and passed to `sandbox-exec
-f`.
On Linux, the Landlock ruleset is constructed in memory.
On Windows, the job object is created once and reused.

Path resolution happens at profile generation time: relative paths in
`access.fs` rules are resolved against the workspace root.
Symlinks are resolved to avoid sandbox bypasses via symlink traversal.

## Drawbacks

- **Platform inconsistency.** The three enforcement mechanisms have different
  strengths.
  `sandbox-exec` is capable but deprecated and uses an undocumented profile
  language.
  Landlock is sound but requires kernel 5.13+.
  Windows provides only coarse-grained enforcement in the initial
  implementation.
  Users on different platforms get different levels of protection.

- **`sandbox-exec` deprecation risk.** Apple has deprecated `sandbox-exec` with
  no public replacement.
  All SBPL documentation comes from reverse engineering.
  Apple could remove it in any macOS release, leaving macOS without OS-level
  enforcement until an alternative is developed.
  This is the most significant platform risk since macOS is a primary
  development platform for many JP users.

- **Performance overhead.** Sandbox setup adds latency to tool spawning.
  On macOS, `sandbox-exec` adds a wrapper process.
  On Linux, Landlock ruleset creation adds ~1ms.
  These costs are per-tool-call but small relative to the tool's own execution
  time and LLM latency.

- **False denials.** The default sandbox may break tools that legitimately need
  access beyond the workspace root (e.g., tools that read system headers, access
  package caches, or interact with databases).
  Users must diagnose the sandbox violation and add the appropriate `access`
  entry.
  OS-level denials produce raw permission errors that may be difficult to
  diagnose without context.

- **Configuration burden.** Adding `access` config is additional work for tool
  authors.
  The defaults are designed to cover common cases, but tools with unusual
  requirements (build tools that read toolchain directories, tools that need
  network access) need explicit configuration.

- **Environment variable isolation is a behavioral change.** Today, tool
  subprocesses inherit the full environment.
  This RFD strips all variables except a minimal set.
  Tools relying on `CARGO_HOME`, `RUSTUP_HOME`, proxy settings, or other
  environment variables will need explicit `EnvRule` entries.

## Alternatives

### No OS-level sandboxing (cooperative only)

Rely entirely on [RFD 076]'s cooperative enforcement and a future VFS IPC
protocol for access control.
A future RFD is expected to introduce `runtime = "vfs"` tools that access host
resources through mediated IPC; until that exists, every tool runs as `runtime =
"stdio"` (the current default) and would get no mandatory restrictions beyond
`RunMode`.

Rejected because `runtime = "stdio"` is the default and will remain the most
common mode.
Leaving the majority of tools without mandatory protection defeats the purpose.
Cooperative enforcement is valuable for error messages but does not prevent a
buggy or malicious tool from ignoring the policy.

### Container-based sandboxing

Run each tool in a lightweight container (e.g., a micro-VM or namespace
sandbox).
This provides strong isolation but adds significant complexity and startup
latency, and is not available on all platforms (notably Windows and macOS have
limited namespace support).

Rejected as disproportionate for the threat model.
JP's tools are typically short-lived commands that the user has configured.
The threat is accidental over-reach (a path argument the LLM chose badly), not
adversarial exploitation.
OS-level filesystem restrictions are sufficient for this threat model.

### Seccomp-only on Linux

Use seccomp-bpf instead of Landlock for all Linux enforcement.
Seccomp is more powerful (it can filter any syscall) but also more fragile —
blocking the wrong syscall crashes the process.
The filesystem-level model of Landlock maps directly to `AccessPolicy` and is
safer to use.

Seccomp may be added as a supplementary layer in the future, but Landlock is the
primary mechanism.

## Non-Goals

- **Sandboxing JP itself.** This RFD restricts tool subprocesses, not the JP
  process.
  JP needs full access to the filesystem, network, and process table to
  function.

- **Sandboxing MCP tools.** MCP tools run on external servers.
  The server's security is the server operator's responsibility.
  JP trusts MCP tool results the same way it trusts any network response.

- **Restricting LLM API calls.** The LLM provider connection is not subject to
  the sandbox system.
  It is configured separately via provider settings.

- **Fine-grained syscall filtering.** This RFD targets filesystem, network, and
  subprocess restrictions — the capabilities most relevant to tool execution.
  Low-level syscall filtering (e.g., blocking `ptrace`, `mount`) is out of
  scope.

- **Replacing `RunMode`.** The sandbox complements `RunMode`, it does not
  replace it.
  `RunMode` controls whether a tool runs.
  The sandbox controls what the tool can do while running.

- **Post-hoc detection of OS sandbox violations.** When the OS sandbox denies an
  operation, the tool receives a raw permission error.
  JP cannot intercept OS-level denials or reliably detect them by scanning
  stderr — the heuristic for distinguishing sandbox denials from other
  permission errors is inherently imprecise.
  This RFD does not attempt runtime detection of sandbox violations.
  If a tool fails due to a sandbox restriction, the user diagnoses it from the
  tool's error output and adds the appropriate `access` entry.
  A future RFD may explore detection heuristics or structured error reporting
  from tools.

- **Sanitizing terminal escape sequences in tool output.** Tool results and
  custom formatters can emit arbitrary ANSI control sequences (OSC 52 clipboard
  writes, window-title changes, cursor movement) that reach the user's terminal.
  This is a real tool trust-boundary concern, but it is a *terminal output*
  problem, not a filesystem/network/subprocess one, so it is out of scope for
  this sandbox.
  It is handled by the capability-aware terminal theming work, whose render sink
  strips non-SGR escapes from tool-sourced output by default; a complete
  trust-boundary policy for tool-emitted escapes should land here in a future
  revision once that sink exists.

## Risks and Open Questions

1. **`sandbox-exec` removal timeline.** Apple has deprecated `sandbox-exec` but
   provided no timeline for removal and no public replacement API.
   If removed, macOS falls back to no OS-level enforcement.
   A future RFD should investigate alternative macOS sandboxing before this
   becomes urgent.

2. **Landlock kernel version adoption.** Landlock requires kernel 5.13+.
   Network restrictions require 6.7+.
   What is the minimum kernel version we should target?
   Most modern distributions ship 5.15+ but CI environments and older servers
   may not.

3. **Symlink and mount-point handling.** Sandbox path restrictions use resolved
   absolute paths.
   Symlinks inside the workspace that point outside it create a potential
   bypass.
   JP resolves all symlinks at sandbox setup time.
   Should it also deny symlink traversal to out-of-scope targets?

4. **Convergence with RFD 016.** This RFD proposes adding `CommandRule` to [RFD
   076]'s `AccessPolicy`.
   [RFD 016] independently defines `CommandRule` and `SandboxConfig` for WASM
   plugins.
   Once this RFD is accepted, [RFD 016]'s WASM plugin config should adopt the
   shared `AccessPolicy` types.
   Is this a clean migration, or does the WASM sandbox have requirements that
   `AccessPolicy` doesn't cover?

5. **Windows filesystem restrictions.** The initial Windows implementation
   provides only coarse-grained enforcement (network on/off, child process
   restriction).
   Is this acceptable for a first release, or should Windows support be deferred
   entirely until a more capable mechanism is identified?

6. **Environment variable discovery.** Stripping all env vars by default will
   break tools that depend on undocumented environment variables (toolchain
   paths, proxy settings, editor preferences).
   How do users discover which variables a tool needs?
   Should JP log which variables were stripped when a tool fails?

## Implementation Plan

### Phase 1: `CommandRule` in `AccessPolicy`

- Add `commands: Option<HashMap<String, CommandRule>>` to `AccessPolicy` in
  `jp_tool`, extending [RFD 076]'s types.
- Add `commands` field support to the `access` config in `jp_config`.
- Unit tests for command matching logic.
- **Dependency:** [RFD 076] Phase 1 (types in `jp_tool`).

### Phase 2: Environment variable isolation

- Modify tool subprocess spawning in `jp_llm` to call `env_clear()` and forward
  only the minimal set (`PATH`, `HOME`, `USER`, `LANG`, locale).
- When `access.env` rules are present, forward matching variables in addition to
  the minimal set.
- When no `access` config is present, apply the same minimal-set default.
- Update JP's built-in tools and documentation for any newly required `EnvRule`
  entries.
- **Dependency:** Phase 1.

### Phase 3: macOS `sandbox-exec` integration

- Implement SBPL profile generation from `AccessPolicy`.
- Modify tool subprocess spawning to wrap with `sandbox-exec` when available.
- Handle path resolution and symlink traversal.
- Implement sensitive path warnings.
- Add integration tests that verify sandbox denials (macOS CI only).
- Feature-gate behind `#[cfg(target_os = "macos")]`.
- **Dependency:** Phase 2.

### Phase 4: Linux Landlock integration

- Implement Landlock ruleset generation from `AccessPolicy`.
- Apply ruleset in child process after `fork()` before `exec()`.
- Detect kernel support and fall back gracefully.
- Add integration tests (Linux CI only).
- Feature-gate behind `#[cfg(target_os = "linux")]`.
- **Dependency:** Phase 2.

### Phase 5: Windows best-effort enforcement

- Implement job object creation for network and process restrictions.
- Spawn tool process with job assignment.
- No per-path filesystem restriction in this phase.
- Add integration tests (Windows CI only).
- Feature-gate behind `#[cfg(target_os = "windows")]`.
- **Dependency:** Phase 2.

## References

- [RFD 076] — Tool access grants: defines `AccessPolicy` types, cooperative
  enforcement, filesystem/network/environment access rules.
  This RFD consumes those types for OS-level enforcement and extends them with
  `CommandRule`.
- [RFD 016] — WASM plugin architecture, sandbox configuration model, secret
  scrubbing, inquiry-based permissions.
  This RFD extends its sandboxing concept to subprocess tools.
- [RFD 009] — Stateful tool protocol.
  Sandboxing applies to both one-shot and stateful tool sessions.
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
