# RFD 117: Transparent Tool Call Fan-Out

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-18
- **Extended by**: [RFD 129]
- **Summary**: Adds a per-tool `fan_out` setting so one tool call can batch
  multiple independent operations through JP's MCP server.

## Summary

This RFD adds **fan-out**: a per-tool configuration setting that lets a single
tool call carry multiple independent operations without changing the tool's
implementation.
JP's in-process MCP server ([RFD 109]) advertises an `ops` envelope schema,
expands each call into child invocations under the tool's concurrency and error
policies, and folds their results into a single response.

## Motivation

An assistant that already knows which files it wants to read pays for one
request-response cycle per call:

```text
fs_read_file(path: "docs/pitch.md", start_line: 58, end_line: 68)
fs_read_file(path: "scripts/migrate.sh", start_line: 1928, end_line: 1940)
fs_read_file(path: "scripts/migrate.sh", start_line: 2072, end_line: 2082)
... 11 more
```

Each cycle re-sends the accumulated context.
Cached input costs roughly a tenth of uncached input, so ten cached cycles cost
about as much as one uncached request, but fourteen sequential cycles still add
fourteen round-trips of latency and fourteen turns of the agent loop.

The obvious fix is to give each tool a multi-argument schema, but that approach
has two problems:

**It does not reach external MCP tools.** `jp_mcp::server::resolve_tool` builds
a `ToolDefinition` for local, built-in, and MCP tools alike; for MCP tools, it
takes the upstream server's `inputSchema` and applies configured overrides.
Because JP does not own external MCP schemas, a per-tool schema change only
helps tools implemented in JP.

**It entangles two independent concerns.** What a tool does and how many
operations fit in a single call are separate axes.
Baking batching into each tool's schema forces every new tool to re-implement
it, and adding it to an existing tool requires changing its schema, validation,
and result formatting.

Fan-out moves batching from tool implementation to tool configuration, so it
applies uniformly across local, built-in, and external MCP tools.

## Design

### User-Facing Behavior

Fan-out exists only on the wire; terminal output and permission prompts do not
change.

A call carrying three operations renders as three tool calls, prompts for
permission three times unless the tool runs with `run = "allow"`, and displays
three results:

```text
Calling tool fs_read_file(path: "docs/pitch.md", start_line: 58, end_line: 68)
Calling tool fs_read_file(path: "scripts/migrate.sh", start_line: 1928, end_line: 1940)
Calling tool fs_read_file(path: "scripts/migrate.sh", start_line: 2072, end_line: 2082)
```

A call carrying a single operation, whether wrapped in the `ops` envelope or
sent bare, renders as a single tool call, indistinguishable from a tool without
fan-out.

Each rendered call gets its own approval prompt.
Collapsing three rendered calls into a single prompt would ask the user to
approve something different from what the terminal displayed.
Fan-out reduces provider round-trips rather than user prompts, and the read-only
tools that dominate call volume (`run = "allow"`) do not prompt at all.

### MCP Server Boundary

Fan-out is a property of the tool as advertised by JP's in-process MCP server,
not of any LLM provider or MCP client:

- Every caller obtains tool schemas from `Service::definitions()`, including
  JP's own provider requests, MCP `tools/list` responses for external clients
  such as Claude Code over ACP ([RFD 110]), and the `describe_tools` built-in
  tool.
- Every tool call passes through the server, which expands the envelope
  regardless of which client submitted the call.

Providers forward the schema they receive, so neither `jp_llm` nor
`jp_tool::ToolDefinition` contains fan-out logic.
Any execution path that bypassed the MCP server for a fan-out tool would be a
bug in that path.

### Schema Envelope

When a tool enables `fan_out`, the server wraps its advertised parameter schema
in an `ops` envelope:

```json
{
  "type": "object",
  "properties": {
    "ops": {
      "type": "array",
      "minItems": 1,
      "description": "The operations to perform. Each element is one complete set of this tool's arguments.",
      "items": {
        "<the tool's own schema>": "..."
      }
    }
  },
  "required": [
    "ops"
  ],
  "additionalProperties": false
}
```

The array field is named `ops` rather than `arguments` or `args` because each
element represents a complete **operation**, and the tool's existing parameter
object is already its arguments.

Any `$defs` or `definitions` block in the tool's schema is hoisted to the
envelope root.
Because JSON Schema resolves local references against the document root (for
example, `#/$defs/Name`), leaving those definitions nested under
`properties.ops.items` would break them, and external MCP servers routinely emit
`$defs` blocks.
Self-root references (`$ref: "#"`) are not rewritten.

The server also appends a sentence to the advertised description instructing the
assistant to batch all known operations into a single call.
The tool's existing usage examples are left unchanged because each example shows
a single bare operation, which remains a valid call.

### Bare Calls

If a caller invokes a fan-out tool with bare arguments instead of an `ops`
envelope, the server executes it as an ordinary single call without child
invocations or result framing.
How the server distinguishes an envelope from a bare call depends on whether the
tool's own schema declares a top-level `ops` parameter:

**The tool does not declare `ops`** (the common case).
The presence of an `ops` key always indicates an envelope, and its absence
indicates a bare call.
If `ops` is not a non-empty array of objects, or if an element fails validation,
the server returns an error identifying the invalid operation rather than
falling back to a bare call.
Falling back would mask the real mistake by replacing `"operation 2: unknown
argument pth"` with `"unknown argument ops"`.

**The tool declares `ops`.** The server treats the arguments as an envelope only
if `ops` is a non-empty array of objects and every element validates against a
copy of the tool's schema after type coercion and default injection.
If any check fails, `ops` is treated as the tool's own parameter and the call
runs bare.
When both interpretations validate (which can occur when all tool parameters are
optional, as in `{"ops": [{"dry_run": true}]}`), the envelope interpretation
wins because that is the advertised schema.

The advertised schema always requires `ops`.
Strict tool use on OpenAI and Anthropic rejects schemas with a root-level
`anyOf`, so the server cannot advertise both shapes at the root, and merging the
tool's properties into the envelope as optional fields would strip the tool's
`required` constraints.
Bare calls are therefore accepted by the server without being advertised in the
schema: clients that follow the schema batch operations, and clients that send
bare arguments still succeed.

### Parent and Child Invocations

When `Service::start_call` receives an envelope call for a fan-out tool, it
creates a **parent invocation** and expands each element of `ops` into a **child
invocation**:

- Each **child invocation** is an ordinary invocation with its own
  `InvocationId`, cancellation token (derived from the parent's token),
  execution attempts, and `stderr` progress stream.
  It runs the standard `run_call` state machine against the tool's unwrapped
  schema, so type coercion, defaults, validation, custom argument formatting,
  and questions work per operation without change.
- The **parent invocation** does not execute the tool itself.
  It spawns the children, enforces the concurrency and error policies, waits for
  all children to settle, folds their results, and owns the single `Record`
  barrier and MCP response.

When a tool asks a question and receives an answer, the server re-executes the
invocation.
Because each operation is an independent child invocation, only the child that
asked the question re-runs; already completed operations are never recomputed.

Two Host-side mechanisms work across child invocations without modification:

- **Inquiry IDs do not collide.** `TurnState::next_inquiry_attempt` increments
  per `(tool_id, question_id)`, so two operations asking the same question under
  one tool call ID receive distinct attempt numbers.
- **Turn-scoped answers apply across operations.** Before prompting the user,
  the Host checks `remembered_tool_answers`, keyed by `(tool_name,
  question_id)`.
  An answer saved at `PersistLevel::Turn` resolves subsequent operations in the
  batch automatically while still recording each as its own inquiry pair.

### Host Channel Protocol

Each child invocation communicates with the Host over the same channel and
interaction protocol as a plain call, with three additions:

**Every interaction identifies its operation.** `CallInfo` gains `operation:
Option<Operation>`, where `Operation` carries the parent's `InvocationId`, the
operation's zero-based `index`, and the total operation `count`.
`CallInfo::request` retains the caller's original request, including the `ops`
envelope, so the Host's request-correlation check sees the same request for
every child.
The individual operation's arguments are passed in `Prepare` and `Release`, as
with any call.

**A child ends with `Settled` instead of `Record`.** Because the conversation
stream records a single response per tool call, individual child invocations are
never recorded directly.
Each child finishes with `Interaction::Settled`, which carries a `Recording` of
the operation: its executed arguments, its raw tool result (`None` if the tool
never ran), and the result approved for delivery.
The Host acknowledges `Settled` immediately so the parent can fold the results
without waiting on terminal rendering.

Every child completed by the service sends exactly one `Settled`, including
children skipped under `on_error = "stop"`.
A child resolved directly by the Host via `complete_call` (such as during a
"Stop & respond" interrupt) does not send `Settled`, because the Host already
holds its result.

**The parent ends with `Record`, carrying the folded body.** The parent's
`result` is both persisted to the conversation and returned to the MCP caller.
The Host already received each operation's individual result in `Settled`.

Plain calls, including bare calls to a fan-out tool, carry `operation: None` on
every interaction and use `Record` as usual.

### Scheduling

Every child invocation enters `Prepare` as soon as the parent starts it,
requesting argument formatting and permission immediately so the user approves
all operations in the batch before any operation executes, matching the behavior
of separate calls in a turn.

Concurrency and error policies are enforced between Host release (`Release`) and
tool execution.
Once the Host releases a child, the child waits to acquire a concurrency slot
before running the tool.
When `concurrency` is bounded, children acquire slots in the order they appear
in `ops`, so `concurrency = 1` executes operations in request order regardless
of the order the Host released them.
Once a child acquires a slot, the parent checks the error policy: under
`on_error = "stop"`, if an earlier operation failed, the child settles
immediately as not run instead of executing.

A running child records any failure before releasing its concurrency slot,
preventing the next queued child from starting before the failure is visible.
Failure detection uses the raw tool outcome (including command spawn failures
and upstream MCP errors) before result-mode policy applies, so `result = "skip"`
or a declined `result = "ask"` review cannot mask an error from `on_error =
"stop"`.
If a child terminates without running the tool (because of a Host decision or
argument validation failure), its settled result determines whether it failed.

Because the parent waits on its child invocations rather than on the Host, an
`on_error = "stop"` trigger that skips all remaining operations finishes the
call immediately instead of blocking on work that will never start.

> [!TIP]
> [RFD 129] moves the concurrency limit and execution ordering to the MCP Host,
> applying them at release time across all calls to a tool rather than only
> within a single fan-out call.
> The parent invocation retains only the `on_error = "stop"` check.

### Host Responsibilities

The Host (`jp_cli::cmd::query::tool`) retains its existing responsibilities:
prompting, rendering, routing questions, reviewing results, and recording
events.
It neither schedules operations nor folds results:

- The Host submits a fan-out call like any other tool call.
  Before sending the request, it runs the same `split` helper as the server to
  open one executor per operation before the first interaction arrives, and
  refuses to route an interaction whose operation count disagrees with that
  split.
- It demultiplexes child interactions by `operation.index` to the corresponding
  executor.
  Each operation's display state is keyed by `<tool call id>#<index>`, giving
  each prompt, progress indicator, and result its own terminal line.
- Each operation is tracked as a separate entry in the coordinator's batch:
  announced, prompted for, and released in `ops` order like any standalone tool
  call.
- `Settled` completes an individual operation's executor, while the parent's
  `Record` completes the overall tool call and produces the persisted
  conversation event.
  Once every operation has a response, the Host requests the parent's `Record`
  and marks the batch settled only after `Record` arrives.
- Every operation, regardless of whether its outcome was decided by the tool, a
  permission prompt, a result review, or cancellation, is resolved through
  `Executor::settle`, which answers whichever barrier the child is parked on.
  Unlike a plain call, an operation cannot wait for the `Record` acknowledgement
  because `Record` is sent only after all operations have resolved.
  `Executor::settle` also waits until the underlying MCP request has been sent,
  because an operation settled before its first `Prepare` arrived would
  otherwise wait on a child invocation the server had not yet spawned.
- An operation waiting behind a concurrency limit is simply an executor whose
  `Release` has not yet started the tool, so the Host needs no scheduling queue
  of its own.

### Result Folding

The parent folds child results in the order the operations appear in `ops`, not
the order they finished.

When a fan-out call contains a single operation, its result is recorded bare,
whether it succeeded or failed, so at `N = 1` the envelope leaves no trace in
the output.
When `N > 1`, the folded body frames each operation with an `[i/N]` header and
explicitly lists operations that did not run:

```text
[1/5] ok
File deleted.

[2/5] error
File has uncommitted changes. Please stage or discard first.

[3/5] not run (stopped after operation 2 failed)
[4/5] not run (stopped after operation 2 failed)
[5/5] not run (stopped after operation 2 failed)
```

Without the `not run` lines, the assistant would assume all five operations were
attempted and reason from a false premise.

The overall tool call is recorded as an error only when no operation succeeded
(for example, when operation 1 fails under `on_error = "stop"`).
If at least one operation succeeds, the call is recorded as a success and the
framed sections identify which individual operations failed.
Both the LLM provider and conversation replay use this top-level status to
select error or success formatting.

### Interrupts

The Host routes interrupt actions to the parent invocation and its unfinished
children:

- **Stop & respond** holds every unfinished child (whether running or waiting
  for a concurrency slot) and completes each with a cancellation response.
  The parent folds those cancellation responses with any completed operations,
  recording a single response that shows which operations were cancelled.
- **Restart** pauses unfinished children while preserving the results of
  children that have already settled.
  On resume, only the unfinished children re-enter `Prepare`, so the Host
  re-renders and re-prompts only those operations.
- **Escalating** past the interrupt menu cancels the Host's in-flight attempts;
  every unfinished child settles as cancelled and no further operations start.

### Rendering and Replay

During conversation replay (`jp conversation print`), any recorded envelope call
for a tool with `fan_out` enabled in its turn configuration is re-expanded using
the server's `expand` helper, reproducing the live terminal output.
A recorded call without an `ops` key renders as a single call.
Because replay has the turn configuration but not the tool's JSON Schema, a bare
call to a tool that declares its own top-level `ops` parameter is also expanded
as operations during replay.

Custom argument formatter output is stored as a single string on the
`ToolCallRequest` event, preserving compatibility with conversations recorded
without fan-out.
Each operation's formatted chunk is joined with a newline in `ops` order, and an
operation re-announced after a restart replaces its chunk.
Because the chunks are stored as one combined string, replay prints all
operation headers followed by all descriptions, whereas live output prints each
description directly beneath its header.

### Configuration

`fan_out` accepts either a boolean or a table, matching the pattern used by
`enable` in the same configuration tree:

```toml
# reads: unbounded concurrency, report every failure
[conversation.tools.fs_read_file]
fan_out = true

# writes: one at a time, in order, stop at the first failure
[conversation.tools.fs_delete_file.fan_out]
concurrency = 1
on_error = "stop"

# rate-limited remote calls
[conversation.tools.github_issues.fan_out]
concurrency = 4
```

| Key           | Default    | Meaning                                                      |
| ------------- | ---------- | ------------------------------------------------------------ |
| `concurrency` | unbounded  | Maximum operations in flight. `1` runs them in order.        |
| `on_error`    | `continue` | `stop` starts no further operations after the first failure. |

`concurrency` is a single integer rather than a `mode = "parallel" |
"sequential"` enum paired with `max_concurrency`, avoiding contradictory states
such as `mode = "sequential", max_concurrency = 4`.
A future `delay` setting for rate-limited endpoints can be added alongside these
keys without changing either.

`on_error` is orthogonal to `concurrency`: independent reads use unbounded
concurrency with `continue`, ordered writes use `concurrency = 1` with `stop`,
and the remaining two combinations are also valid.

When `concurrency` is greater than `1`, `on_error = "stop"` prevents new
operations from *starting* after a failure; in-flight operations run to
completion rather than being aborted.

Specifying the table form enables fan-out without requiring `enabled = true`,
while `enabled = false` disables it.
Setting `concurrency = 0` means unbounded concurrency.
`fan_out` is a per-tool setting with no counterpart on the
`conversation.tools.*` wildcard block, so no tool inherits it by default:
whether a tool's calls can be safely batched depends on the tool's semantics.

> [!TIP]
> [RFD 129] replaces `fan_out.concurrency` with a tool-level `concurrency`,
> which counts separate calls and operations against one limit and can be shared
> across tools by key.
> `fan_out` keeps `enabled` and `on_error`.

### Interactive Tools and Ordering

An earlier draft of this design restricted fan-out to tools that never return
`NeedsInput`, because a question raised mid-batch leaves earlier side effects
committed.

That restriction is unnecessary because `concurrency` and `on_error` express the
required safety property directly.
A tool configured with `concurrency = 1` and `on_error = "stop"` behaves
identically to separate sequential calls: if operation 2 asks a question,
operation 1 has committed and operations 3 through 5 have not started.
That is the exact state the assistant would reach by issuing individual calls.

Safety for stateful tools depends on whether execution is ordered, not on
whether the tool can ask questions, and the tool's configuration controls that
ordering.

## Drawbacks

**Two batching mechanisms.** `fs_modify_file` already accepts multiple targets
in one call, and `bash` accepts multiple commands.
Because fan-out neither replaces nor subsumes tool-native batching (see
Non-Goals), JP carries two mechanisms for executing multiple actions in a single
call.

**The advertised schema differs from `ToolDefinition`.** `jp_tool::schema`
states that a tool's parameters are stored exactly as declared by the source.
That remains true of `ToolDefinition`, but the MCP server advertises a synthetic
wrapper schema for fan-out tools.

**More complex Host protocol.** `CallInfo::operation` and `Interaction::Settled`
expand the Host interaction protocol, and every Host implementation (not just
JP's terminal UI) must demultiplex one MCP call across multiple executors.

**Replay loses per-operation boundaries in custom-formatter output.** Because
per-operation formatter chunks are joined into a single string on
`ToolCallRequest`, replay reproduces the formatted text but cannot associate
each chunk with its individual operation header.

## Alternatives

**Per-tool multi-argument schemas.** Add array parameters such as `reads[]` on
`fs_read_file` or `files[]` on `fs_create_file`.
Rejected because it cannot wrap external MCP tools and forces each tool to
implement its own batching schema, result framing, and partial-failure handling.

**Expand envelopes in the Host.** Advertise the envelope to the provider and
have the Host split each call into separate MCP calls before submission.
This was the initial implementation, and was rejected because it depends on who
submits calls.
When an external agent submits calls directly to the MCP server ([RFD 110]), the
Host never sees them before the server does, so the agent can only be shown the
tool's plain schema and fan-out is unavailable on that route.
Host-side expansion also records a folded response that no MCP call actually
returned, requiring per-operation acknowledgements to reconcile the two.

**Loop over operations inside a single invocation.** Rejected because the server
re-executes a tool from the beginning after a `NeedsInput` question is answered,
which would re-run committed side effects from earlier operations, and because a
single invocation carries only one Host approval whereas fan-out requires one
approval per operation.
Separate child invocations isolate both retries and approvals per operation.

**Restrict fan-out to tools that cannot ask questions.** Rejected in favor of
the `concurrency` and `on_error` settings, which preserve sequential safety
guarantees without excluding interactive write tools.

## Non-Goals

**Replacing atomic batch operations.** `fs_modify_file` performs one coordinated
operation across a set of targets rather than `N` independent operations: its
patterns apply in sequence across files so each sees the previous pattern's
output, it presents a single combined diff for one approval, and it validates
the entire set before writing to disk.
Tools with cross-target semantics opt out by leaving `fan_out` unset.

**Shared parameters across operations.** Every element of `ops` carries a
complete argument object.
Hoisting shared default values to the call root (similar to the call-level
`path` default on `fs_modify_file`) is left to a future extension.

**A rate-limit delay setting.** Rate-limited endpoints may eventually need a
delay between operations.
The table configuration leaves room for a `delay` key, but this RFD does not
implement it.

**Changing tool implementations.** Enabling `fan_out` requires no changes to a
tool's implementation, and each operation executes identically to a standalone
call.

## Risks and Open Questions

**Model adoption of the envelope schema.** Wrapping a tool's schema adds
nesting, and some models may continue issuing one operation per call, paying the
extra wrapper tokens without batching.
Enabling `fan_out` on `fs_read_file` first and measuring the operation-count
distribution (tracked in T-0yavtsr) will test model behavior before enabling the
setting on additional tools.

**Permission fatigue on write tools.** Combining fan-out with a write tool that
prompts before execution produces `N` prompts for `N` operations.
Users can mitigate this by remembering their approval decision for the rest of
the turn, which approves the remaining operations without prompting, though
real-world usage will determine whether that is ergonomic enough on write tools.

**Client-side schema validation.** An MCP client that validates arguments
against `inputSchema` before sending a request will reject a bare call that JP's
server would accept.
Clients that follow the advertised schema are unaffected, so bare-call support
is a server-side leniency rather than a promise to every caller.

## Implementation Plan

### Phase 1: Configuration

Add `FanOutConfig` to `jp_config::conversation::tool` with bool-or-table
parsing, `concurrency`, and `on_error`.
No runtime behavior yet.

**Depends on:** nothing.
**Mergeable:** yes.

### Phase 2: Advertisement

Add `jp_mcp::server::fan_out` with `envelope`, `split`, and `fold`, where
`split` distinguishes an envelope from a bare call as described under Bare
Calls.
Update `Service::definitions()` to advertise the envelope schema and batching
instruction, and source JP's provider tool definitions from
`Service::definitions()`.
`jp_tool` and `jp_llm` carry no fan-out code.

**Depends on:** Phase 1.
**Mergeable:** no; a tool advertised with the envelope needs Phase 3 to run.

### Phase 3: Child Invocations

Add `CallInfo::operation` and `Interaction::Settled`.
Expand envelope calls into child invocations under a parent invocation, enforce
`concurrency` and `on_error` between release and execution, and fold child
results into the parent's `Record`.
Execute bare calls as plain calls.

**Depends on:** Phase 2.
**Mergeable:** with Phase 4.

### Phase 4: Host

Demultiplex child interactions into one executor per operation, key display
state by operation, complete operations on `Settled` and the tool call on the
parent's `Record`, and resolve every operation with `Executor::settle`.
Route pause, resume, and cancel signals to the appropriate parent or child
invocation.

**Depends on:** Phase 3.
**Mergeable:** with Phase 3.

## References

- [RFD 082] records each tool question round-trip as an inquiry pair, keeping
  `N` per-operation questions individually auditable under a single tool call
  ID.
- [RFD 109] runs every tool call through JP's in-process MCP server, where
  fan-out is implemented.
- [RFD 110] lets external agents submit tool calls over ACP; those calls reach
  the same MCP server and fan out the same way.

[RFD 082]: 082-unified-inquiry-event-recording.md
[RFD 109]: 109-in-process-jp-mcp-server.md
[RFD 110]: 110-anthropic-subscription-queries-via-acp.md
[RFD 129]: 129-tool-concurrency-limits.md
