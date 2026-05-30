# RFD D44: MCP Server Sandboxing

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-30
- **Extends**: [RFD 075]
- **Requires**: [RFD 076]

## Summary

This RFD extends [RFD 075]'s OS-level sandboxing to stdio MCP servers.
JP spawns stdio MCP servers as child processes, so it can confine them with the
same platform-native mechanisms [RFD 075] applies to local tools.
A new `stateful` flag on each server's configuration determines both its process
lifecycle and where its access policy is declared: a `stateful` server (the
default) runs once for the whole agentic loop and is confined by a server-wide
policy; a stateless server starts and stops per tool call and is confined by a
per-tool policy.
The relationship is enforced symmetrically — declaring access at the wrong
level is a config error, not a silent lie.
Unconfigured servers receive a default sandbox (workspace read-write, no
network, allowlisted environment).

## Motivation

[RFD 075] sandboxes local subprocess tools but lists "sandboxing MCP tools" as a
Non-Goal, on the reasoning that "MCP tools run on external servers."
That reasoning conflates two cases.
It is true for a hypothetical remote/HTTP transport — but JP has no such
transport.
The only MCP transport JP implements is `stdio`, and a stdio server is a **child
process JP spawns itself**, in `jp_mcp::Client::create_client`, using the same
`tokio::process::Command` machinery that [RFD 075] sandboxes for local tools.
JP controls that spawn.
The Non-Goal is an accident of framing, not a law of physics.

Leaving stdio servers unconfined is a real gap.
A local MCP server is an arbitrary binary — typically `npx -y some-server` or a
small Python or Rust process — running with the user's full privileges.
Once the user approves a tool call, nothing stops the server reading
`~/.ssh/id_rsa`, exfiltrating workspace contents over the network, or writing
outside the workspace.
The empirical norm is exactly the shape JP can confine: a small local binary
exposing a handful of tools.
Sandboxing those servers is what lets a user trust an assistant to invoke tools
without auditing every server's source.

## Design

### Two axes: confinement and authorization

Tool safety has two independent questions, and conflating them is what makes MCP
awkward:

| Axis                    | Question                            | Attaches to     | Surface                                  |
| ----------------------- | ----------------------------------- | --------------- | ---------------------------------------- |
| **Process confinement** | What can the running process touch? | the **process** | `access` ([RFD 075] / [RFD 076]) |
| **Call authorization**  | Should JP send / prompt this call?  | the **call**    | argument-conditional tool policy         |

For a local tool, the process *is* the call — each invocation is its own spawn
— so both axes collapse onto `[conversation.tools.X]` and nobody notices they
differ.
For an MCP tool they split: confinement attaches to the server process, while
authorization attaches to each call.
Call authorization for MCP tools is already handled — argument-conditional tool
policy (`policy.run` / `policy.result`, defined in a separate draft RFD)
evaluates each call's arguments before JP sends it, and works identically for
MCP and local tools because the arguments are available as JSON either way.
**This RFD is only about the confinement axis.**

### The `stateful` flag

MCP is a session-oriented protocol: a server runs an `initialize` handshake and
may hold state across calls (an open browser, a database connection, a built
index).
Whether a server is stateful determines whether JP can give it a dedicated
per-call process — which in turn determines whether per-tool confinement is
even meaningful.

A new `stateful` field on `StdioConfig` captures this, defaulting to `true`
(today's behavior):

```toml
[providers.mcp.github]
command = "npx"
arguments = ["-y", "@modelcontextprotocol/server-github"]
# stateful = true (default): one process for the whole agentic loop
```

- **`stateful = true`** — the server runs once per `jp query` and persists
  across tool calls, as today.
  There is one process, shared by all the server's tools.
- **`stateful = false`** — the server starts and stops per tool call.
  Each call gets a fresh, dedicated process.

A stateless server pays the `initialize` round-trip and process-startup cost on
every call.
Most local MCP servers are cheap enough for this to be unnoticeable; for those
that aren't, `stateful = true` keeps the persistent process at the cost of
per-tool confinement (see below).
The flag is the single knob a user turns to trade boot cost against confinement
granularity.

### Where access is declared, and the honesty rule

Because OS confinement applies to a *process*, the access policy must attach to
whatever process exists.
This is enforced symmetrically, as a hard config error in both directions:

| Server             | Access declared on                                       | The other surface is                    |
| ------------------ | -------------------------------------------------------- | --------------------------------------- |
| `stateful = true`  | `providers.mcp.<server>.access` (one persistent process) | per-tool `access` → **config error**    |
| `stateful = false` | `conversation.tools.<tool>.access` (per-call process)    | server-wide `access` → **config error** |

The error is the point.
A `stateful` server runs all its tools in one process, so a per-tool filesystem
grant cannot be enforced — `tool_a` read-only and `tool_b` read-write would
share one profile, and "`tool_a` is read-only" would be a lie.
JP refuses to let the config express it, and the error names the correct
surface:

```
error: `conversation.tools.github_search.access` is not enforceable on a tool
       from a stateful MCP server.

  The `github` server runs as one shared process, so per-tool confinement
  cannot be applied. Declare access for the whole server:

      [providers.mcp.github.access]

  Or set `stateful = false` on the server to give each tool call its own
  sandboxed process.
```

Conversely, a stateless server has no persistent process for a server-wide
profile to bind to — only N ephemeral per-call processes — so server-wide
`access` is rejected and access is declared per tool.

This RFD therefore amends [RFD 076], which today rejects `access` on *all* MCP
tools at config load.
That flat rejection becomes the `stateful`-conditional rule above.

[RFD 076]'s cross-layer merge (append across config files via `MergeableVec`) is
unchanged.
This RFD deliberately does **not** add cross-scope inheritance (a server default
that per-tool config overrides); see [Non-Goals](#non-goals).
A multi-tool stateless server repeats grants per tool in V1.

### Default policy and allowlist semantics

The sandbox applies even with no `access` block.
Unconfigured servers receive:

| Capability  | Default                                                                         |
| ----------- | ------------------------------------------------------------------------------- |
| Filesystem  | Workspace root, read-write                                                      |
| Network     | Denied                                                                          |
| Environment | Minimal set (`PATH`, `HOME`, `USER`, `LANG`, locale) plus allowlist             |
| Commands    | Unrestricted (subprocess filtering is opt-in via [RFD 075]'s `CommandRule`) |

All three resource axes are **allowlist / default-deny**, matching [RFD 075] and
[RFD 076].
Environment is not exempt: the server's environment is *cleared* and rebuilt
from the minimal set plus the variables the user explicitly grants.
This is a deliberate uniformity and safety choice — environment is the axis
secrets live on (tokens, API keys), and a leaked secret is the breach that
cannot be walked back.
A denylist would silently expose every new secret added to the user's
environment; an allowlist fails closed.

The environment allowlist reuses the existing `providers.mcp.<server>.variables`
field, whose meaning sharpens once the environment is cleared: today it is
redundant (the child inherits everything), and under this RFD it becomes the
actual passthrough list.
Its relationship to [RFD 076]'s `access.env` is an open reconciliation (see
[Risks](#risks-and-open-questions)).

### Profile generation: the `jp_sandbox` boundary

[RFD 075] generates platform-native profiles (macOS `sandbox-exec`, Linux
Landlock, Windows job objects) from an `AccessPolicy`.
That generator is needed at two spawn sites now — `jp_llm` (local tools) and
`jp_mcp` (servers) — so it lives in a shared `jp_sandbox` module consumed by
both, rather than welded to the local-tool path.
The generator's input is an `AccessPolicy` and a workspace root; its output is a
platform profile / command wrapper.
`jp_mcp` wraps the server `Command` exactly as `jp_llm` wraps a local-tool
`Command`.
No MCP-specific profile logic is introduced.

### Conveying the policy to the server

The OS sandbox is the enforcement boundary, but a cooperative server produces
better errors if it knows its limits.
JP advertises the resolved access policy to the server over the MCP protocol's
auxiliary channel — `_meta` on the `initialize` request for the full policy,
and the standard `roots` capability for the filesystem boundary.
This mirrors [RFD 076], where a local tool receives its `AccessPolicy` in the
`Context` JSON; an MCP server receives the same information through the
transport-appropriate channel.
It is advisory: a well-behaved server self-limits and self-reports denials
precisely; a server that ignores it is still contained by the OS.

JP's MCP client currently serves connections as the unit handler (`()`), which
advertises no client capabilities.
Using the protocol channel requires replacing it with a handler that advertises
`roots` and attaches `_meta` — a contained but real prerequisite.

### Diagnosing sandbox failures

When the OS denies an operation, a third-party server returns whatever error it
chooses, and JP cannot always know a sandbox caused it.
JP does control the transport, though, which gives a usable classification:

| Failure stage                           | JP observes                                                  | Sandbox attribution            |
| --------------------------------------- | ------------------------------------------------------------ | ------------------------------ |
| Spawn/exec fails                        | `CannotSpawnProcess` (already captured)                      | High — profile may deny exec   |
| Dies before `initialize` completes      | connection drop + stderr-tail ring buffer (already captured) | High — boot-time access denied |
| Returns a tool error after `initialize` | error content                                                | Low — heuristic hint only      |

The high-confidence cases reuse machinery JP already has (the stderr tail is
attached to `InitializeError` today).
The low-confidence case appends a clearly labelled hint when the error matches
permission signatures (`EACCES`, `EPERM`, "permission denied", "connection
refused").
Cooperating servers — including JP's own `grizzly` and `bookworm` —
self-report precisely via the `_meta` channel above.
This section describes the intent; the implementor decides how much to build in
V1 and what to defer.
Reliable attribution for arbitrary third-party servers is not promised.

## Drawbacks

- **Per-call latency for stateless servers.** `stateful = false` pays process
  startup plus the `initialize` handshake on every call.
  In a multi-call agentic turn this compounds.
  The flag lets the user opt out, but the default for a server the user wants
  confined per-tool is the slower path.

- **The default sandbox breaks servers on first run.** Default-deny network and
  a cleared environment mean a `github`-style server (needs network) or any
  Node/Python server (needs environment beyond the minimal set) fails until the
  user adds grants.
  This is the cost of failing closed.
  It is mitigated by the diagnostics above and by the fix being a one-line
  `variables` / `access` addition, not blind investigation — but it is real
  friction, and a default painful enough to disable defeats itself.

- **Verbosity for multi-tool stateless servers.** With no inheritance in V1, a
  stateless server exposing several tools repeats its grants per tool.

- **Platform inconsistency, inherited from [RFD 075].** macOS `sandbox-exec` is
  deprecated, Linux Landlock needs kernel 5.13+ (6.7+ for network), Windows is
  coarse.
  MCP servers inherit the same uneven protection and the same fallback (warn and
  run unconfined where no mechanism exists).

## Alternatives

### Server-level access only, no `stateful` flag

Attach `access` solely to `providers.mcp.<server>` and never to MCP tools.
This is simpler but cannot express per-tool confinement at all — a user who
wants `tool_a` read-only and `tool_b` read-write on the same server has no way
to say so.
The `stateful` flag exists precisely to make that expressible (and truthful) for
servers that can tolerate per-call lifecycle.

### Per-tool access merged into one server profile

Keep per-tool `access` on every MCP tool and apply the union of a server's
tools' policies to the shared process.
Rejected because it lies: the union is what every tool actually gets, so
`tool_a` read-only silently becomes read-write the moment `tool_b` requests it.
The granularity is illusory, and presenting per-tool config that silently unions
violates least astonishment.

### Spawn every MCP server per call, unconditionally

Make per-call lifecycle the only mode, so per-tool confinement is always
truthful.
Rejected because it breaks stateful servers (a browser opened in one call is
gone the next) and contradicts JP's existing persistent-server design ([RFD
009], [RFD 037]).
The `stateful` flag preserves the persistent default and makes per-call an
opt-in for servers that tolerate it.

### Do not sandbox MCP at all (authorization + trust only)

Treat MCP purely as a trust boundary — connection approval plus call
authorization — and never OS-confine the server, the way some other clients do.
Rejected because user safety is a first-class goal here.
JP spawns the process; leaving it unconfined when the mechanism to confine it
already exists is a choice to leave the user exposed.
Call authorization gates what JP *sends*; it does not stop a server from
touching the filesystem once a legitimate call is sent.

### Cross-scope inheritance for stateless servers

Allow a server-wide default that per-tool config inherits and overrides
(`tools.*.access` → `providers.mcp.X.access` → `tools.X.access`).
Deferred to a future iteration.
It is genuine convenience for multi-tool stateless servers but introduces a
cross-scope merge composed on top of [RFD 076]'s cross-layer merge — the most
intricate part of the design — for a V1 that works without it.

## Non-Goals

- **Remote / non-stdio MCP transports.** JP cannot OS-confine a process it does
  not spawn.
  If an HTTP or other remote transport is added later, [RFD 075]'s reasoning
  genuinely applies to it, and confinement is the server operator's
  responsibility.

- **Cross-scope access inheritance.** Per-tool config does not inherit from a
  server-wide default in V1 (see Alternatives).
  Grants are declared at exactly one level, determined by `stateful`.

- **Serializing parallel MCP calls.** Avoiding concurrent spawns of the same
  stateless server (which a mismarked stateless server could turn into a
  resource conflict) is deferred.
  `stateful = false` is a contract the user asserts: the server holds no
  cross-call or cross-instance state.

- **Replacing call authorization.** Argument-conditional tool policy remains the
  per-call authorization layer for MCP tools.
  This RFD does not touch it.

- **Sandboxing JP itself, or MCP resource/prompt fetches.** Only tool-executing
  server processes are confined.

## Risks and Open Questions

1. **`variables` vs `access.env`.** Once the environment is cleared, the
   existing server-level `variables` field and [RFD 076]'s `access.env` are two
   spellings of the same allowlist, and under the symmetric rule a stateless
   server's env grants must live per tool like its other grants.
   The cleanest resolution is `access.env` absorbing `variables` (with
   `variables` as deprecated sugar), but it needs to be specified.

2. **Discovery spawn for stateless servers.** Enumerating a stateless server's
   tools needs a spawn that is not tied to any one tool.
   It runs under the default baseline; a server that needs more than that merely
   to list its tools should be `stateful = true`.
   The implementor should confirm this is sufficient in practice.

3. **Client capability advertisement.** The protocol channel requires replacing
   the `()` client handler with one advertising `roots` / `_meta`.
   Scope of that change in `jp_mcp` needs validation.

4. **Default-on migration.** Default-deny network and cleared environment will
   break existing users' servers on upgrade.
   Is a warn-only first release (generate the profile, log what *would* be
   denied, enforce nothing) worth the transitional safety gap, or is immediate
   enforcement plus good diagnostics the better trade?

5. **`sandbox-exec` / Landlock platform limits**, inherited from [RFD 075]: the
   macOS deprecation risk and the Landlock kernel-version floor apply equally to
   MCP server confinement.

## Implementation Plan

### Phase 1: `stateful` flag and symmetric validation

Add `stateful: bool` (default `true`) to `StdioConfig` in `jp_config`.
Amend [RFD 076]'s MCP-`access` rejection into the `stateful`-conditional rule:
per-tool `access` on a tool whose server is stateful is a config error; a
server-wide `access` on a stateless server is a config error.
Both errors name the correct surface.
Unit tests for both directions.

Depends on [RFD 076] (access types and the existing rejection it amends).
Can merge independently of the sandbox itself.

### Phase 2: `jp_sandbox` extraction

Extract [RFD 075]'s profile generators into a shared `jp_sandbox` module
consuming `(AccessPolicy, workspace_root)` and producing a platform profile /
command wrapper.
Re-point `jp_llm`'s local-tool path at it (behavior-preserving).

Depends on [RFD 075] implementation.
This is a refactor of 075's code, not new behavior.

### Phase 3: Environment allowlist

In `jp_mcp`'s server spawn, `env_clear()` and rebuild from the minimal set plus
the configured allowlist.
Reconcile `variables` with `access.env` per Risk 1.

Depends on Phase 1.

### Phase 4: Confine stateful servers

Apply the `jp_sandbox` profile to the persistent server `Command` in
`create_client`, generated from `providers.mcp.<server>.access` (or the default
policy when absent).

Depends on Phases 2 and 3.

### Phase 5: Stateless per-call lifecycle

For `stateful = false`, spawn a fresh confined process per tool call from the
tool's per-tool `access`, run the one call, and tear it down.
Cache discovered tool schemas so the per-call path does not re-enumerate.

Depends on Phase 4.

### Phase 6: Cooperative policy channel and diagnostics

Replace the `()` client handler with one advertising `roots` / `_meta`, and
attach the resolved policy.
Add the transport-stage failure classification and annotations described above;
defer low-value pieces to V2 at the implementor's discretion.

Depends on Phase 4 (policy resolution) and Phase 5 (per-call attribution).

## References

- [RFD 075] — OS-level sandboxing for subprocess tools.
  This RFD reuses its profile generators (via a shared `jp_sandbox` module) and
  corrects its "sandboxing MCP tools" Non-Goal for the stdio transport.
- [RFD 076] — Tool access grants.
  This RFD consumes its `AccessPolicy` types and amends its flat rejection of
  `access` on MCP tools into the `stateful`-conditional rule.
- [RFD 016] — WASM plugin architecture and sandbox model.
- [RFD 009] — Stateful tool protocol; the persistent-server model this RFD's
  `stateful = true` default preserves.
- [RFD 037] — Await tool for stateful handle synchronization.
- [Model Context Protocol — Roots] — the client capability used to convey the
  filesystem boundary to a server.
- [Model Context Protocol — `_meta`] — the auxiliary field used to convey the
  full access policy.

[Model Context Protocol — Roots]: https://modelcontextprotocol.io/docs/concepts/roots
[Model Context Protocol — `_meta`]: https://modelcontextprotocol.io/specification/2025-06-18/basic#meta
[RFD 009]: ../009-stateful-tool-protocol.md
[RFD 016]: ../016-wasm-plugin-architecture.md
[RFD 037]: ../037-await-tool-for-stateful-handle-synchronization.md
[RFD 075]: ../075-tool-sandbox-and-access-policy.md
[RFD 076]: ../076-tool-access-grants.md
