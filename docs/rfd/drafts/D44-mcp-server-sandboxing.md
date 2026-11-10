# RFD D44: MCP Server Sandboxing

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-30
- **Extends**: [RFD 075]
- **Requires**: [RFD 076]
- **Summary**: Extends OS-level subprocess sandboxing to stdio MCP servers with
  `stateful` server-wide and stateless per-tool access policies.

## Summary

This RFD extends [RFD 075]'s OS-level subprocess sandboxing to stdio MCP servers
spawned by JP.
A `stateful` flag on each server's configuration controls both its process
lifecycle and where its access policy is declared: stateful servers (the
default) persist across the turn loop under a server-wide policy, while
stateless servers spawn per tool call under a per-tool policy.
Unconfigured servers run in a default sandbox with read-write workspace access,
no network access, and an allowlisted environment.

## Motivation

[RFD 075] sandboxes local subprocess tools but lists "sandboxing MCP tools" as a
Non-Goal because "MCP tools run on external servers."
That reasoning holds for a remote HTTP transport, which JP does not implement.
The only MCP transport JP supports is `stdio`, and a stdio server is a child
process that `jp_mcp::Client::create_client` spawns with the same
`tokio::process::Command` machinery [RFD 075] sandboxes for local tools.

Leaving stdio servers unconfined exposes the host system.
A local MCP server is an arbitrary binary (typically `npx -y some-server` or a
small Python or Rust process) running with the user's full privileges.
Once the user approves a tool call, nothing stops the server from reading
`~/.ssh/id_rsa`, exfiltrating workspace contents over the network, or writing
outside the workspace.
Sandboxing these local server processes lets users run MCP tools without
auditing every server's source code.

## Design

### Confinement and authorization

Tool safety separates into two independent concerns:

| Axis                    | Question                            | Attaches to     | Surface                                  |
| ----------------------- | ----------------------------------- | --------------- | ---------------------------------------- |
| **Process confinement** | What can the running process touch? | the **process** | `access` ([RFD 075] / [RFD 076]) |
| **Call authorization**  | Should JP send or prompt this call? | the **call**    | argument-conditional tool policy         |

For a local tool, each invocation spawns its own process, so the process and the
call are 1:1 and both axes live under `[conversation.tools.X]`.
For an MCP tool, the two axes split: confinement attaches to the server process,
while authorization attaches to each tool call.
Argument-conditional tool policy (`policy.run` and `policy.result`, defined in a
separate draft RFD) already handles call authorization for both local and MCP
tools by evaluating each call's JSON arguments before execution.
This RFD covers only process confinement.

### The `stateful` flag

MCP is a session-oriented protocol: a server runs an `initialize` handshake and
may hold state across calls, such as an open browser, a database connection, or
an in-memory index.
Whether a server holds cross-call state determines whether JP can spawn a fresh
process per call, which in turn determines whether per-tool confinement is
possible.

A `stateful` field on `StdioConfig` controls this lifecycle and defaults to
`true`:

```toml
[providers.mcp.github]
command = "npx"
arguments = ["-y", "@modelcontextprotocol/server-github"]
# stateful = true (default): one process for the whole agentic loop
```

- **`stateful = true`**: JP spawns one server process per `jp query` and reuses
  it across tool calls.
  All tools on the server share that process.
- **`stateful = false`**: JP starts and stops the server for each tool call.
  Every call runs in a fresh, dedicated process.

A stateless server pays process startup and the `initialize` round-trip on every
call.
For servers where that startup cost is too high, `stateful = true` keeps a
persistent process and applies confinement at the server level instead of per
tool.

### Where access policies are declared

Because OS confinement applies to a process, the access policy must be
configured at the level where a process exists.
JP enforces this symmetrically at config load time:

| Server             | Access declared on                                       | Rejected surface                        |
| ------------------ | -------------------------------------------------------- | --------------------------------------- |
| `stateful = true`  | `providers.mcp.<server>.access` (one persistent process) | per-tool `access` → **config error**    |
| `stateful = false` | `conversation.tools.<tool>.access` (per-call process)    | server-wide `access` → **config error** |

A stateful server runs all of its tools in a single process, so per-tool
filesystem grants cannot be enforced: if `tool_a` declared read-only access and
`tool_b` declared read-write access, both would share one OS sandbox profile.
Rather than silently widening `tool_a`'s permissions, JP rejects per-tool
`access` on stateful servers and points to the valid configuration key:

```
error: `conversation.tools.github_search.access` is not enforceable on a tool
       from a stateful MCP server.

  The `github` server runs as one shared process, so per-tool confinement
  cannot be applied. Declare access for the whole server:

      [providers.mcp.github.access]

  Or set `stateful = false` on the server to give each tool call its own
  sandboxed process.
```

Conversely, a stateless server has no persistent process, only ephemeral
per-call processes, so JP rejects server-wide `access` and requires access to be
declared per tool.

This rule amends [RFD 076], which rejects `access` on all MCP tools at config
load.
[RFD 076]'s cross-layer merge (`MergeableVec` appending across config files) is
unchanged.
Per-tool configurations do not inherit from a server-wide default (see
[Non-Goals](#non-goals)); in V1, a multi-tool stateless server repeats its
grants on each tool.

### Default policy and allowlist semantics

Sandboxing applies even when no `access` block is configured.
Unconfigured servers receive the following baseline policy:

| Capability  | Default                                                                         |
| ----------- | ------------------------------------------------------------------------------- |
| Filesystem  | Workspace root, read-write                                                      |
| Network     | Denied                                                                          |
| Environment | Minimal set (`PATH`, `HOME`, `USER`, `LANG`, locale) plus allowlist             |
| Commands    | Unrestricted (subprocess filtering is opt-in via [RFD 075]'s `CommandRule`) |

Filesystem, network, and environment access all follow the allowlist
(default-deny) model from [RFD 075] and [RFD 076].
Before spawning a server, JP clears its environment and rebuilds it from the
minimal set plus the variables the user explicitly grants.
Because API keys and tokens live in environment variables, a denylist would
expose any new secret added to the shell environment; an allowlist fails closed.

The environment allowlist reuses `providers.mcp.<server>.variables`.
Without environment clearing, that field is redundant because child processes
inherit the full parent environment; with clearing enabled, `variables` becomes
the explicit passthrough list.
Reconciling `variables` with [RFD 076]'s `access.env` remains an open question
(see [Risks](#risks-and-open-questions)).

### Shared profile generation in `jp_sandbox`

[RFD 075] generates platform-native sandbox profiles (macOS `sandbox-exec`,
Linux Landlock, and Windows job objects) from an `AccessPolicy`.
Because both `jp_llm` (local tools) and `jp_mcp` (MCP servers) spawn sandboxed
subprocesses, profile generation lives in a shared `jp_sandbox` module consumed
by both crates.
The generator takes an `AccessPolicy` and a workspace root and returns a
platform profile and command wrapper.
`jp_mcp` wraps the server `Command` the same way `jp_llm` wraps a local-tool
`Command`, with no MCP-specific profile logic.

### Conveying the policy to the server

The OS sandbox enforces the boundary, but a cooperative server can produce
clearer error messages when it knows its limits up front.
JP advertises the resolved access policy during the MCP handshake: the full
policy goes in `_meta` on the `initialize` request, and the filesystem boundary
uses the standard `roots` client capability.
This mirrors [RFD 076], which passes `AccessPolicy` to local tools in the
`Context` JSON.
The advertised policy is advisory: a cooperative server can check it and report
denials directly, while a server that ignores it remains confined by the OS.

JP's MCP client currently uses the unit handler (`()`), which advertises no
client capabilities.
Supporting this channel requires replacing `()` with a client handler that
advertises `roots` and attaches `_meta` to `initialize`.

### Diagnosing sandbox failures

When the OS blocks an operation, an arbitrary third-party server returns its own
error message, so JP cannot always attribute a failure to the sandbox.
However, the transport stage at which the failure occurs narrows down the cause:

| Failure stage                            | JP observes                                                  | Sandbox attribution           |
| ---------------------------------------- | ------------------------------------------------------------ | ----------------------------- |
| Spawn or exec fails                      | `CannotSpawnProcess` (already captured)                      | High: profile may deny exec   |
| Process exits before `initialize`        | connection drop + stderr-tail ring buffer (already captured) | High: boot-time access denied |
| Tool returns an error after `initialize` | error content                                                | Low: heuristic hint only      |

The high-confidence stages reuse existing error reporting, including the stderr
tail attached to `InitializeError`.
For tool errors after initialization, JP appends a sandbox hint when the error
text matches common permission signatures (`EACCES`, `EPERM`, "permission
denied", or "connection refused").
Cooperative servers, including JP's `grizzly` and `bookworm`, can report sandbox
denials directly using the `_meta` policy described above.
Reliable attribution for arbitrary third-party servers is not guaranteed, and
the implementor can decide how much of the heuristic matching to include in V1.

## Drawbacks

- **Per-call latency for stateless servers.** Setting `stateful = false` pays
  process startup and the `initialize` handshake on every call, which compounds
  across multi-call turns.
  Users who want per-tool confinement must accept this startup overhead.

- **First-run failures under the default sandbox.** Because network access is
  denied and the environment is cleared by default, servers that need network
  access (such as `github`) or extra environment variables (such as Node or
  Python runtime paths) fail until the user configures grants.
  Clear diagnostics and one-line `variables` or `access` fixes reduce this
  friction, but a strict default still interrupts initial setup.

- **Repeated configuration for multi-tool stateless servers.** Without
  cross-scope inheritance in V1, a stateless server that exposes several tools
  must repeat shared access grants on each tool.

- **Platform inconsistency, inherited from [RFD 075].** macOS `sandbox-exec` is
  deprecated, Linux Landlock requires kernel 5.13+ (6.7+ for network filtering),
  and Windows job objects are coarse-grained.
  MCP servers inherit the same platform gaps and fallback behavior (warn and run
  unconfined when no sandbox mechanism is available).

## Alternatives

### Server-level access only, no `stateful` flag

Attach `access` only to `providers.mcp.<server>` and never to individual MCP
tools.
This is simpler, but it cannot express per-tool confinement when a user wants
`tool_a` to be read-only and `tool_b` to be read-write on the same server.
The `stateful` flag makes per-tool confinement possible for servers that can run
one process per call.

### Per-tool access merged into one server profile

Allow per-tool `access` on all MCP tools and apply the union of those policies
to the shared server process.
Rejected because the union grants every tool the highest privilege requested by
any sibling tool: a read-only `tool_a` silently gains write access as soon as
`tool_b` requests it.
Per-tool settings that silently widen into a shared union are misleading.

### Spawn every MCP server per call

Run every MCP server as a per-call process so per-tool confinement always
applies.
Rejected because it breaks servers that hold state across calls (such as an open
browser session) and conflicts with JP's persistent-server model ([RFD 009],
[RFD 037]).
Defaulting to `stateful = true` preserves persistent sessions while letting
stateless servers opt into per-call isolation.

### Rely on call authorization alone

Treat MCP purely as a trust and authorization boundary without OS-level process
confinement.
Rejected because call authorization only controls which tool calls JP sends; it
cannot prevent an approved call from reading arbitrary files or making network
requests once the server executes it.
Because JP spawns stdio servers directly, it can enforce OS-level limits on them
using the same sandbox infrastructure built for local tools.

### Cross-scope inheritance for stateless servers

Allow a server-wide default policy that per-tool configurations inherit and
override (`tools.*.access` → `providers.mcp.X.access` → `tools.X.access`).
Deferred to a future iteration.
While convenient for multi-tool stateless servers, combining cross-scope
inheritance with [RFD 076]'s cross-layer config merge adds complexity that V1
does not need.

## Non-Goals

- **Remote or non-stdio MCP transports.** JP cannot apply an OS sandbox to a
  process it does not spawn.
  For HTTP or other remote transports, process confinement remains the server
  operator's responsibility.

- **Cross-scope access inheritance.** Per-tool config does not inherit from a
  server-wide default in V1 (see [Alternatives](#alternatives)).
  Grants are declared at a single level determined by `stateful`.

- **Serializing parallel MCP calls.** Preventing concurrent spawns of the same
  stateless server is deferred.
  Setting `stateful = false` asserts that the server holds no cross-call or
  cross-instance state.

  A future `exclusive` flag on the server configuration can address servers that
  cannot run concurrently: at most one instance runs at a time, and any turn
  that needs the server waits for the active holder to finish, whether in the
  same process or another `jp` process.
  `exclusive` is orthogonal to `stateful`: `stateful` controls how long an
  instance lives, while `exclusive` controls how many instances can run at once.
  A server that locks a database file, browser profile, or fixed port requires
  exclusivity, and contention becomes more likely once a long-running host
  spawns one instance per distinct configuration and working directory ([RFD
  113]).
  Specifying `exclusive` requires resolving four design questions:

  - **Lock scope.** Server names are config keys, so two workspaces can use the
    same name for different servers.
    The lock must key on the underlying resource scope (per machine, per
    workspace, or per checkout).
  - **Wait surface.** Because a lease can last for an entire interactive turn,
    waiting turns need a visible, interruptible progress indicator across the
    terminal, the plugin protocol, and other frontends.
  - **Deadlock prevention.** If a turn uses multiple exclusive servers,
    acquiring locks in inconsistent orders across turns can deadlock.
    Lock acquisition needs a deterministic order that composes with the
    conversation lock the turn already holds.
  - **Cross-process locking.** Serializing only within a single process would
    still allow conflicts across concurrent `jp` invocations, so the initial
    implementation must use a cross-process lock (`jp_storage::ResourceLocker`).

- **Replacing call authorization.** Argument-conditional tool policy remains the
  per-call authorization layer for MCP tools.

- **Sandboxing JP itself, or MCP resource and prompt fetches.** Only
  tool-executing server processes are confined.

## Risks and Open Questions

1. **`variables` vs. `access.env`.** Once the environment is cleared, the
   server-level `variables` field and [RFD 076]'s `access.env` both configure
   the environment allowlist.
   Under the placement rule, a stateless server's environment grants must live
   on each tool alongside its other grants.
   Having `access.env` absorb `variables` (keeping `variables` as a deprecated
   alias on stateful servers) is the cleanest resolution, but the exact
   migration needs to be specified.

2. **Tool discovery spawn for stateless servers.** Listing a stateless server's
   tools (`tools/list`) requires an initial spawn that is not tied to any single
   tool.
   This discovery spawn runs under the default baseline policy; a server that
   needs extra permissions just to list its tools must use `stateful = true`.
   Implementation should verify that baseline permissions suffice for common
   servers.

3. **Client capability advertisement.** Sending `roots` and `_meta` requires
   replacing the `()` client handler in `jp_mcp` with a custom handler, and the
   scope of that change needs validation.

4. **Default-on migration.** Denying network access and clearing the environment
   by default will break existing MCP server configurations on upgrade.
   We need to decide whether to ship a warn-only release first (generating the
   profile and logging what would be denied without enforcing it) or enforce the
   sandbox immediately and rely on error diagnostics.

5. **Platform sandbox limits.** Inherited from [RFD 075]: macOS `sandbox-exec`
   deprecation and Linux Landlock kernel-version requirements apply equally to
   MCP server confinement.

## Implementation Plan

### Phase 1: `stateful` flag and symmetric validation

Add `stateful: bool` (default `true`) to `StdioConfig` in `jp_config`.
Replace [RFD 076]'s flat rejection of MCP `access` with `stateful`-conditional
validation: reject per-tool `access` when the server is stateful, and reject
server-wide `access` when the server is stateless.
Both errors name the valid configuration path, with unit tests covering both
directions.

Depends on [RFD 076] and can merge independently of the sandbox implementation.

### Phase 2: `jp_sandbox` extraction

Extract [RFD 075]'s profile generators into a shared `jp_sandbox` module that
takes `(AccessPolicy, workspace_root)` and produces a platform profile and
command wrapper.
Update `jp_llm`'s local-tool execution path to use `jp_sandbox` without changing
behavior.

Depends on [RFD 075].

### Phase 3: Environment allowlist

Call `env_clear()` when spawning MCP servers in `jp_mcp` and rebuild the
environment from the minimal baseline plus the configured allowlist.
Reconcile `variables` with `access.env` as described in Risk 1.

Depends on Phase 1.

### Phase 4: Confine stateful servers

In `create_client`, wrap the persistent server `Command` in a `jp_sandbox`
profile generated from `providers.mcp.<server>.access` (or the default baseline
policy when unset).

Depends on Phases 2 and 3.

### Phase 5: Stateless per-call lifecycle

When `stateful = false`, spawn a fresh sandboxed server process for each tool
call using the tool's `access` policy, execute the call, and terminate the
process.
Cache discovered tool schemas so per-call execution does not repeat tool
discovery.

Depends on Phase 4.

### Phase 6: Cooperative policy channel and diagnostics

Replace the `()` client handler with a handler that advertises `roots` and
attaches the resolved policy in `_meta`.
Add transport-stage failure classification and error hints, deferring optional
heuristics to V2 at the implementor's discretion.

Depends on Phases 4 and 5.

## References

- [RFD 075]: OS-level sandboxing for subprocess tools.
  This RFD reuses its profile generators (via a shared `jp_sandbox` module) and
  revises its "sandboxing MCP tools" Non-Goal for the stdio transport.
- [RFD 076]: Tool access grants.
  This RFD uses its `AccessPolicy` types and replaces its flat rejection of
  `access` on MCP tools with the `stateful`-conditional rule.
- [RFD 016]: WASM plugin architecture and sandbox model.
- [RFD 009]: Stateful tool protocol; the persistent-server model that `stateful
  = true` preserves by default.
- [RFD 037]: Await tool for stateful handle synchronization.
- [Model Context Protocol: Roots][Model Context Protocol — Roots]: Client
  capability used to convey the filesystem boundary to a server.
- [Model Context Protocol: `_meta`][Model Context Protocol — `_meta`]:
  Auxiliary field used to convey the full access policy.

[Model Context Protocol — Roots]: https://modelcontextprotocol.io/docs/concepts/roots
[Model Context Protocol — `_meta`]: https://modelcontextprotocol.io/specification/2025-06-18/basic#meta
[RFD 009]: ../009-stateful-tool-protocol.md
[RFD 016]: ../016-wasm-plugin-architecture.md
[RFD 037]: ../037-await-tool-for-stateful-handle-synchronization.md
[RFD 075]: ../075-tool-sandbox-and-access-policy.md
[RFD 076]: ../076-tool-access-grants.md
[RFD 113]: ../113-context-decomposition-for-invocation-and-workspace-scope.md
