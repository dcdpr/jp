# RFD 128: Tool Concurrency Limits

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-31
- **Extends**: [RFD 117]
- **Summary**: A per-tool `concurrency` limit, shareable across tools by key,
  bounds and orders separate calls and fan-out operations alike.

## Summary

A tool's `concurrency` setting limits how many of its executions run at once,
counting separate calls and fan-out operations alike, and starts them in the
order the assistant wrote them.
Tools that name the same concurrency key share one limit.
The setting moves here from `fan_out`, which keeps only `on_error`.

## Motivation

`ToolCoordinator::release` releases every approved call of a batch at once, and
the calls execute in parallel.
That suits reads.
It breaks tools whose calls depend on state an earlier call changed.

Observed with the `jp-rfd` tools, all in one response:

```txt
rfd_renumber(id: "008", new: "011")
rfd_renumber(id: "007", new: "010")
rfd_renumber(id: "006", new: "009")
rfd_renumber(id: "005", new: "008")
...
```

Each step needs the one before it to have freed its target.
In parallel, `005 → 008` races `008 → 011` and fails.
Later in the same session, three `rfd_draft` calls each scanned the drafts
directory, each picked draft `02`, and each wrote a file under that id;
`create_new` did not catch it because the slugs differ.

Two properties are missing:

1. **Exclusion.** Calls that touch the same state must not overlap.
2. **Order.** They run in the order the assistant wrote them.
   The assistant wrote the renumber chain in that order because each step
   depends on the previous one.

[RFD 117] gives both to the operations of one fanned-out call, through
`fan_out.concurrency`, but the limit stops at the call.
`fs_delete_file` with `fan_out = { concurrency = 1 }` orders its writes only
when the assistant puts them in one envelope; two separate calls to it still run
in parallel.
A reader of that configuration would expect the tool's writes to be ordered.

The limit protects a resource: state on disk, or an endpoint's rate limit.
The resource does not care whether its executions came from one envelope or from
several calls, or from one tool or several: `rfd_draft` and `rfd_renumber` both
allocate draft slots, and `github_issues` and `github_pulls` share one API
quota.
The limit belongs on the tool, shareable across tools, not on fan-out.

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

Unbounded is the default: a tool without a limit has every call and operation
released together once the batch is approved, as today.

A **concurrency key** names the resource a limit protects.
Every execution of every tool with the same key counts against one limit.

- The configured name is the tool's key under `conversation.tools`, the name the
  assistant calls it by.
  Keys and tool names share one namespace, so `{ key = "fs_delete_file" }` on
  another tool joins `fs_delete_file`'s limit.
- A limit of `0` is unbounded.
  A config layer cannot unset a key a lower layer set ([RFD 038] resets whole
  configurations, not single keys), so `concurrency = 0` is how a later layer
  lifts a limit, such as one a plugin's shipped definition declares.
  `{ key = "x", limit = 0 }` is accepted and limits nothing.
- Every tool naming a key must give it the same limit.
  A disagreement is a config error that names the tools involved.
  The check covers every configured tool, enabled or not, so enabling a tool
  never turns a valid configuration invalid.
- `concurrency` is a per-tool key with no counterpart on
  `conversation.tools.'*'`: whether a tool's executions conflict depends on what
  the tool does, the reasoning [RFD 117] gives for `fan_out`.

The `jp-rfd` plugin's shipped definitions declare `concurrency = { key = "rfd"
}` on every tool that writes: `draft`, `promote`, `rename`, `renumber`,
`abandon`, `extend`, `require`, and `supersede`.
`list` and `show` leave it unset.

### `fan_out`

`fan_out` keeps `enabled` and `on_error`:

```toml
fan_out = true
fan_out = { on_error = "stop" }
```

`on_error = "stop"` starts no further operation after one fails, so it only has
something to stop when operations wait for each other.
With an unbounded `concurrency`, every operation starts at release and `stop`
does nothing.
That combination is a config error rather than a silent no-op.

Fan-out has not shipped, so `fan_out.concurrency` is removed outright, with no
alias or migration.

### Scheduling

Within a batch, the tool calls of one response, where each operation of a
fanned-out call is its own entry in the order the assistant wrote it:

1. Once every call is approved or settled, calls without a concurrency key are
   released together, as today.
2. A call with a key is released when fewer than `limit` executions with that
   key are running, and every earlier call with that key has been released or
   settled.
   Until then it stays parked, shown as queued.
3. An execution holds its slot from release until it ends, with a result or a
   failure.
   A question the tool asks mid-run keeps the slot: the tool may have done part
   of its work, and another call slipping in would see half of it.
4. When an execution ends, the next eligible call is released.

A failed call does not stop later calls with its key.
Each is a separate call the assistant made and gets its own response, as today.
Stopping at the first failure is what `fan_out.on_error` is for, within one
call.

Result review (`result = "ask"` or `"edit"`) happens after the slot is freed.
The tool has finished by then; review changes what the assistant is told, not
what the tool did.

Cancelling the batch settles parked calls with their cancellation response, like
any call that was never released.
Eligibility is derived from each call's current state, not from marks recorded
along the way, so a restart, which prepares every call without a response again,
releases them in the written order again.

### Where it is enforced

The limit and the order are enforced by the MCP Host, at release.
`on_error = "stop"` stays with the JP MCP Server.

- **Only the Host knows the order.** The batch keeps calls in the order the
  assistant wrote them.
  The server sees invocations in the order their MCP requests arrive, and the
  Host submits each from its own task, so that order is not deterministic.
- **Release is already the Host's decision.** Every invocation, whether JP or an
  agent submitted it, waits at `Interaction::Release` until the Host replies
  `ReleaseDecision::Execute`.
  This RFD changes when the Host sends that reply, not who sends it.
- **No protocol change.** The server's Host interactions stay as they are.

[RFD 117] put fan-out in the server because a call an agent submits reaches the
server without passing the Host first.
Release passes through the Host on every route, so that argument does not apply
to scheduling.

The server's fan-out `Gate` loses its limit and its ordering and keeps the
`stop` check: an operation released after an earlier one failed is not run.
This relies on the server recording an operation's failure before the Host
learns that its execution ended, which `run_call` already does.

The ordering marks the `Gate` keeps today are what ticket `0ya5qnf` (a restart
can reorder sequential fan-out operations) is about.
Removing them removes that bug, provided the Host's scheduling does not
reintroduce it (see Scheduling).

### Changes by crate

- **`jp_config`**: `concurrency` on `ToolConfig`, accepting an integer or a `{
  key, limit }` table, with its `AssignKeyValue`, `PartialConfigDelta`, and
  `ToPartial` arms, and an accessor on `ToolConfigWithDefaults` that resolves it
  to a key and limit.
  `concurrency` removed from `FanOutConfig` and `FanOut`.
  Validation: one limit per key, and `on_error = "stop"` only with a limit of at
  least `1`.
- **`jp_mcp`**: the fan-out `Gate` keeps only the `stop` check.
- **`jp_cli`**: `ToolCoordinator::release` releases what is eligible and parks
  the rest in `Wait::Release`; the step handler releases the next eligible call
  when an execution ends.
  `abandon_parked` already settles calls in `Wait::Release` on cancellation.
- **`jp-rfd`**: the shipped tool definitions, and this repository's copies under
  `.jp/mcp/tools/rfd/`.

## Drawbacks

**Slower batches.** Executions sharing a key run within the limit.
That is the point, but a limit on a read tool costs time and buys nothing.

**One process only.** Two `jp query` processes in one workspace still race.
Calls from different turns never overlap, since a turn's calls finish before the
next request is sent, so the gap is between processes, not between turns.

**A second Host would repeat it.** Any future Host that executes calls has to
implement the same scheduling.
Today the terminal coordinator is the only one.

**Configuration, not declaration.** An MCP tool's author cannot set a limit on
it; the user does, in configuration.
Command plugins can, because they ship their definitions.

**The limit is repeated on every tool that shares a key**, and the agreement
check is what keeps the copies honest.
See Alternatives for the shape that avoids it.

## Alternatives

**Keep `fan_out.concurrency` and add a separate `exclusive` key.** The previous
version of this draft.
Rejected: `exclusive = true` is `concurrency = 1` with a wider scope, so the two
are one axis wearing two names, the shape [RFD 117] rejected for `mode` +
`max_concurrency`.
`exclusive` alongside `fan_out = { concurrency = 4 }` leaves the `4` dead, and
`fan_out.concurrency = 1` alone keeps looking like it orders the tool.

**Define each key once, at `conversation.tools.concurrency.<key>`, and have
tools name it.** One place for each limit, and no agreement check.
Rejected: `conversation.tools` is a map of tool names, so this reserves one, as
[RFD 055] does for `groups`.
It also splits a tool's definition: `jp-rfd` ships one file per tool, and a file
naming a key defined elsewhere no longer stands alone.
The call flips if a key grows settings beyond its limit, such as a delay between
executions.

**Locking inside each tool.** Covers two processes, which this RFD does not.
Rejected as the general mechanism: it gives no ordering, and every stateful tool
re-implements it.
A tool that needs cross-process safety can still add it.

**A scheduler in the JP MCP Server.** Enforced whatever Host is attached, and
testable without a terminal.
Rejected for now: it needs the order, which only the Host has, so the Host would
have to pass a predecessor with each release (`ReleaseDecision::Execute { after:
Option<InvocationId> }`).
The call flips when a second Host that executes calls exists.

**Infer conflicts from `access` grants.** Grants are optional, and an absent one
means unrestricted, so every such tool would conflict with every other.
The draft-slot race is two reads of a directory listing, not two writes to one
path, so overlapping write grants would not catch it either.

**Tool groups ([RFD 055]).** Groups classify tools for `--tools` and
`--no-tools`.
Reusing them would make "disabled by `--no-tools write`" and "shares a limit"
one declaration, when they vary independently.

## Non-Goals

- **Limits across processes.** See Drawbacks.
- **Stopping later calls after a failure.** See Scheduling.
- **Rate limiting by time.** A delay between executions fits as another field of
  the table form, `{ key, limit, delay }`, without disturbing these.
- **A default on `conversation.tools.'*'`.**

## Risks and Open Questions

**Restart.** Scheduling assumes a restart re-prepares the calls without a
response as a batch in the written order, which the coordinator's restart path
appears to do; implementation must confirm it with the ticket `0ya5qnf` test,
ported to the coordinator.

**Calls an agent submits.** Under [RFD 110] an agent submits its own calls.
This RFD assumes they enter the batch in the order the agent wrote them; that is
unverified.
An agent that waits for each call before sending the next already serializes
them, so the risk is limited to agents that submit in parallel.

## Implementation Plan

**Phase 1: tool-level limits.** `concurrency` on `ToolConfig` with its accessor,
validation, and tests, and the Host scheduling in `ToolCoordinator`.
`fan_out.concurrency` stays for now, so nothing regresses.
Coordinator tests: order within a key, the limit within a key, keys running in
parallel with each other, a question keeping the slot, a failure releasing the
next call, cancellation settling parked calls, fan-out operations counting
against their tool's key, and restart order.

**Phase 2: remove `fan_out.concurrency`.** Remove it from `FanOutConfig` and
`FanOut`, strip the `Gate` to the `stop` check, add the `on_error` validation,
and update the tests and schema snapshot that use it.
Close ticket `0ya5qnf`.
Depends on phase 1.

**Phase 3: `jp-rfd`.** Declare `concurrency = { key = "rfd" }` on the writing
tools in the shipped definitions and in `.jp/mcp/tools/rfd/`.
Depends on phase 1.

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
