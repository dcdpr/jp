# RFD 129: Tool Concurrency Limits

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-31
- **Extends**: [RFD 117]
- **Summary**: A per-tool `concurrency` limit, shareable across tools by key,
  bounds and orders separate calls and fan-out operations alike.

## Summary

A tool's `concurrency` setting limits how many of its executions run
concurrently across separate calls and fan-out operations, starting them in the
order the assistant wrote them.
Tools that share a concurrency key share a single limit.
This setting replaces `fan_out.concurrency`, leaving `fan_out` with `enabled`
and `on_error`.

## Motivation

`ToolCoordinator::release` releases every approved call in a batch at once, so
all calls in the batch execute in parallel.
Parallel execution works well for read-only tools, but breaks tools whose calls
depend on state modified by an earlier call in the same batch.

For example, the assistant emitted these `jp-rfd` calls in a single response:

```txt
rfd_renumber(id: "008", new: "011")
rfd_renumber(id: "007", new: "010")
rfd_renumber(id: "006", new: "009")
rfd_renumber(id: "005", new: "008")
...
```

Each call requires the previous call to free its target ID first.
When run in parallel, `005 → 008` races with `008 → 011` and fails.
Later in the same session, three parallel `rfd_draft` calls scanned the drafts
directory, selected draft slot `02`, and wrote separate files with that ID;
`create_new` did not catch the collision because each call used a different
slug.

Two properties are missing:

1. **Exclusion.** Calls that mutate the same state must not overlap.
2. **Order.** Calls must run in the order the assistant wrote them, because
   later steps in a sequence such as the renumber chain depend on earlier ones.

[RFD 117] provides both properties for the operations within a single fanned-out
call via `fan_out.concurrency`, but that limit applies only within a single
call.
Configuring `fs_delete_file` with `fan_out = { concurrency = 1 }` serializes
deletions only when the assistant batches them into one fan-out envelope; two
separate `fs_delete_file` calls still run in parallel, even though the
configuration implies that all deletions are serialized.

A concurrency limit protects an underlying resource, such as state on disk or an
API rate limit.
Contention is the same whether executions come from one fan-out envelope,
separate calls to the same tool, or calls to different tools: `rfd_draft` and
`rfd_renumber` both allocate draft slots, and `github_issues` and `github_pulls`
share one API quota.
Concurrency limits therefore belong on the tool configuration, shareable across
tools, rather than on `fan_out`.

## Design

### Configuration

```toml
# One at a time, in the order the assistant wrote them, calls and operations
# alike. The operations of one call stop at the first failure.
[conversation.tools.fs_delete_file]
concurrency = 1
fan_out = { on_error = "stop" }

# State shared across tools: one execution among all of them.
[conversation.tools.rfd_draft]
concurrency = { key = "rfd" }

[conversation.tools.rfd_renumber]
concurrency = { key = "rfd" }

# An endpoint shared across tools: four executions among all of them.
[conversation.tools.github_issues]
concurrency = { key = "github", limit = 4 }

[conversation.tools.github_pulls]
concurrency = { key = "github", limit = 4 }
```

| Value            | Concurrency key            | Limit     |
| ---------------- | -------------------------- | --------- |
| unset, `0`       | none                       | unbounded |
| `N`              | the tool's configured name | `N`       |
| `{ key, limit }` | `key`                      | `limit`   |
| `{ key }`        | `key`                      | `1`       |

By default, concurrency is unbounded: once a batch is approved, all calls and
operations for a tool without a limit are released together.

A **concurrency key** identifies the shared resource protected by a limit.
All executions of tools with the same key count against a single shared limit.

- A tool's configured name is its key under `conversation.tools` (the name the
  assistant uses to call it).
  Concurrency keys and tool names share a single namespace, so setting `{ key =
  "fs_delete_file" }` on another tool shares `fs_delete_file`'s limit.
- A limit of `0` means unbounded.
  Because a configuration layer cannot unset a single key set by a lower layer
  ([RFD 038] resets entire configuration sections, not individual keys), setting
  `concurrency = 0` lets a later layer remove a limit declared in a lower layer
  or a plugin's shipped tool definition.
  `{ key = "x", limit = 0 }` is also valid and imposes no limit.
- All tools that reference the same concurrency key must specify the same limit.
  Conflicting limits produce a configuration error naming the tools involved.
  Validation checks all configured tools, whether enabled or disabled, so
  enabling a tool never invalidates a valid configuration.
- `concurrency` is configured per tool and has no wildcard counterpart on
  `conversation.tools.'*'`.
  As with `fan_out` in [RFD 117], whether executions conflict depends on what
  each specific tool does.

The `jp-rfd` plugin's shipped definitions set `concurrency = { key = "rfd" }` on
every mutating tool (`draft`, `promote`, `rename`, `renumber`, `abandon`,
`extend`, `require`, and `supersede`), and leave `list` and `show` unset.

### `fan_out`

`fan_out` keeps `enabled` and `on_error`:

```toml
fan_out = true
fan_out = { on_error = "stop" }
```

`on_error = "stop"` prevents remaining operations from starting after a failure,
which only has an effect when operations are queued behind a concurrency limit.
When `concurrency` is unbounded, all operations start at once on release and
`stop` is a no-op, so combining `on_error = "stop"` with unbounded concurrency
is rejected as a configuration error.

Because fan-out has not yet shipped, `fan_out.concurrency` is removed outright
without an alias or migration.

### Scheduling

A batch consists of the tool calls from a single assistant response, with each
operation of a fanned-out call treated as its own entry in the order the
assistant wrote it.
Scheduling within a batch works as follows:

1. Once every call in the batch is approved or settled, calls without a
   concurrency key are released together.
2. A call with a concurrency key is released when fewer than `limit` executions
   with that key are running and every earlier call with that key has been
   released or settled.
   Until then, the call remains parked and is displayed as queued.
3. An execution holds its concurrency slot from release until it finishes with a
   result or an error.
   If the tool prompts the user for input mid-execution, it retains its slot:
   the tool may have already partially mutated state that a concurrent call
   would observe.
4. When an execution finishes, the next eligible call for that key is released.

A failed call does not abort subsequent calls with the same key; each separate
call made by the assistant still runs and receives its own response.
Aborting remaining work after a failure applies only to operations within a
single fanned-out call via `fan_out.on_error`.

Result review (`result = "ask"` or `"edit"`) runs after the concurrency slot is
released.
By that point the tool has finished executing, and review only changes the
response returned to the assistant, not the tool's side effects.

Cancelling the batch settles parked calls with a cancellation response, just
like any other unreleased call.
Release eligibility is computed from each call's current state rather than from
bookkeeping recorded during execution.
If the batch is restarted, all calls without a response are prepared again and
released in their original written order.

### Host-Side Enforcement

The MCP Host enforces concurrency limits and execution order at release time,
while the JP MCP Server continues to enforce `on_error = "stop"`.

- **Only the Host knows the call order.** The Host stores the batch in the order
  the assistant wrote the calls.
  Because the Host spawns a separate task for each invocation, MCP requests
  arrive at the server in non-deterministic order.
- **Release is already controlled by the Host.** Every invocation, whether
  submitted by JP or by an agent, waits at `Interaction::Release` until the Host
  replies with `ReleaseDecision::Execute`.
  Enforcing concurrency in the Host only changes when that reply is sent.
- **No protocol changes.** The interaction protocol between the server and the
  Host is unchanged.

[RFD 117] placed fan-out in the server because tool calls submitted by an
external agent reach the server without going through the Host first.
However, every invocation still pauses at the Host for release approval, so
Host-side scheduling covers both routes.

The server's fan-out `Gate` drops its concurrency limit and ordering logic,
retaining only the `stop` check: if an earlier operation in the fan-out call has
failed, a newly released operation is skipped.
This relies on `run_call` recording an operation's failure before notifying the
Host that the execution has finished, which it already does.

Removing the stateful ordering marks from `Gate` also resolves ticket `0ya5qnf`,
where restarting a batch could reorder sequential fan-out operations, because
Host-side scheduling derives eligibility from current call state rather than
stored marks (see Scheduling).

### Changes by Crate

- **`jp_config`**: Add `concurrency` to `ToolConfig` (accepting an integer or a
  `{ key, limit }` table) with `AssignKeyValue`, `PartialConfigDelta`, and
  `ToPartial` implementations, plus an accessor on `ToolConfigWithDefaults` that
  resolves the setting to a key and limit.
  Remove `concurrency` from `FanOutConfig` and `FanOut`.
  Validate that every tool sharing a key specifies the same limit, and that
  `on_error = "stop"` is only used when `concurrency` is at least `1`.
- **`jp_mcp`**: Simplify the fan-out `Gate` to retain only the `on_error =
  "stop"` check.
- **`jp_cli`**: Update `ToolCoordinator::release` to release only eligible calls
  and park the rest in `Wait::Release`, and update the step handler to release
  the next eligible call when an execution completes.
  `abandon_parked` already settles calls in `Wait::Release` on cancellation.
- **`jp-rfd`**: Set `concurrency = { key = "rfd" }` on mutating tools in the
  plugin's shipped tool definitions and in this repository's copies under
  `.jp/mcp/tools/rfd/`.

## Drawbacks

**Slower batch execution.** Serializing or bounding executions increases
wall-clock time, and misconfiguring a limit on a read-only tool adds latency
without benefit.

**Single-process scope.** Concurrency limits apply only within a single process;
two `jp query` processes running concurrently in the same workspace can still
race.
Within a single process, calls from different turns never overlap because all
calls in a turn complete before the next request is sent.

**Host-specific implementation.** Any future Host that executes tool calls must
implement the same scheduling logic.
Currently, the terminal `ToolCoordinator` is the only Host.

**Configured by the user, not the MCP server.** The MCP protocol does not let an
external MCP server declare a concurrency limit on its tools, so users must
configure limits in JP.
Command plugins avoid this limitation because they ship their own JP tool
definitions.

**Duplicated limits across shared keys.** Every tool that shares a concurrency
key must repeat the `limit` value, relying on configuration validation to catch
mismatches (see Alternatives).

## Alternatives

**Keep `fan_out.concurrency` and add a separate `exclusive` key.** `exclusive =
true` is equivalent to `concurrency = 1` applied across calls as well as fan-out
operations, splitting one concept across two settings in the same way `mode` and
`max_concurrency` did before [RFD 117] unified them.
Combining `exclusive = true` with `fan_out = { concurrency = 4 }` makes the `4`
a no-op, while setting `fan_out.concurrency = 1` alone still falsely implies
that separate calls to the tool are serialized.

**Define shared keys centrally at `conversation.tools.concurrency.<key>`.**
Defining each key in one table would avoid repeating `limit` across tools and
eliminate the agreement check.
However, `conversation.tools` is a map of tool names, so adding `concurrency`
there would reserve another tool name alongside `groups` ([RFD 055]).
It would also split self-contained tool definitions across files: `jp-rfd` ships
one configuration file per tool, and a file that references an externally
defined key can no longer stand alone.
If concurrency keys later gain additional settings, such as a delay between
executions, a central table may become worthwhile.

**Locking inside each tool.** In-tool locking would protect against concurrent
`jp query` processes, which coordinator scheduling does not.
However, OS locks do not preserve the assistant's call order, and every stateful
tool would have to implement its own locking.
Individual tools that require cross-process safety can still add their own
locks.

**Schedule executions in the JP MCP Server.** Server-side scheduling would apply
regardless of which Host is attached and could be tested without a terminal.
However, only the Host knows the order in which the assistant emitted the calls,
so the Host would have to pass an explicit predecessor with each release
(`ReleaseDecision::Execute { after: Option<InvocationId> }`).
If JP adds a second Host that executes tool calls, moving the scheduler to the
server may become worthwhile.

**Infer conflicts from `access` grants.** Access grants are optional, and an
omitted grant means unrestricted access, so any tool without explicit grants
would conflict with every other tool.
Even with explicit grants, the `rfd_draft` slot race is caused by two calls
reading a directory listing before creating distinct filenames, which
overlapping write-path detection would not catch.

**Reuse tool groups ([RFD 055]).** Tool groups classify tools for CLI selection
via `--tools` and `--no-tools`.
Coupling concurrency limits to tool groups would conflate CLI selection (such as
`--no-tools write`) with execution contention, even though the two concerns vary
independently.

## Non-Goals

- **Cross-process concurrency limits.** See Drawbacks.
- **Aborting subsequent calls after a failure.** See Scheduling.
- **Time-based rate limiting.** A minimum delay between executions can be added
  later as another field on the table form (`{ key, limit, delay }`) without
  changing the existing syntax.
- **Wildcard defaults on `conversation.tools.'*'`.**

## Risks and Open Questions

**Batch restart order.** Host-side scheduling assumes that restarting a batch
re-prepares all unanswered calls in their original written order.
The coordinator's restart path appears to preserve this order, which
implementation must verify by porting the ticket `0ya5qnf` regression test to
the coordinator.

**Agent-submitted tool calls.** Under [RFD 110], an external agent submits its
own tool calls.
This design assumes those calls enter the batch in the order the agent wrote
them, which has not yet been verified.
Because an agent that waits for each call before sending the next already
serializes its executions, this risk applies only to agents that submit calls in
parallel.

## Implementation Plan

**Phase 1: Tool-level concurrency limits.** Add `concurrency` to `ToolConfig`
with its accessor, validation, and tests, and implement Host-side scheduling in
`ToolCoordinator`.
Retain `fan_out.concurrency` during this phase to avoid regressions.
Add coordinator tests covering execution order within a key, concurrency limits
within a key, parallel execution across distinct keys, slot retention while
prompting the user, slot release after a failure, cancellation of parked calls,
fan-out operations counting against their tool's key, and batch restart order.

**Phase 2: Remove `fan_out.concurrency`.** Remove `concurrency` from
`FanOutConfig` and `FanOut`, reduce the fan-out `Gate` to the `stop` check, add
the `on_error` validation, and update the tests and schema snapshot that use it.
Close ticket `0ya5qnf`.
Depends on Phase 1.

**Phase 3: Configure `jp-rfd` tools.** Declare `concurrency = { key = "rfd" }`
on the mutating tools in the shipped definitions and in `.jp/mcp/tools/rfd/`.
Depends on Phase 1.

Once implemented, add "Concurrency Key" to the ubiquitous language.

## References

- [RFD 117]: fan-out and its per-call concurrency policy.
- [RFD 109]: the JP MCP Server and its Host interactions.
- [RFD 055]: tool groups.
- [RFD 038]: config reset keywords, which reset whole configurations.
- `crates/jp_cli/src/cmd/query/tool/coordinator.rs`: `ToolCoordinator::release`.
- `crates/jp_mcp/src/server/service.rs`: `Interaction::Release` and the fan-out
  `Gate`.

[RFD 038]: 038-config-reset-keywords.md
[RFD 055]: 055-tool-groups.md
[RFD 109]: 109-in-process-jp-mcp-server.md
[RFD 110]: 110-anthropic-subscription-queries-via-acp.md
[RFD 117]: 117-transparent-tool-call-fan-out.md
