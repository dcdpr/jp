# RFD 117: Transparent Tool Call Fan-Out

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-18
- **Summary**: A per-tool `fan_out` setting lets one tool call carry several
  independent operations, which JP's MCP server runs separately and folds into
  one response.

## Summary

This RFD adds **fan-out**: a per-tool configuration flag that lets a single tool
call carry several independent operations, which JP executes as if the assistant
had issued them separately.
The tool's own implementation is unchanged.

Fan-out lives entirely in JP's in-process MCP server ([RFD 109]), the one place
every tool call passes through regardless of who submits it.
The server advertises the tool with an envelope schema, expands a call's
envelope into one child invocation per operation, runs them under the tool's
concurrency and error policy, and folds their results into the one response the
caller is waiting for.
No LLM provider, and no MCP client, needs to know fan-out exists.

## Motivation

An assistant that already knows which files it wants pays for one
request-response cycle per call:

```text
fs_read_file(path: "docs/pitch.md", start_line: 58, end_line: 68)
fs_read_file(path: "scripts/migrate.sh", start_line: 1928, end_line: 1940)
fs_read_file(path: "scripts/migrate.sh", start_line: 2072, end_line: 2082)
... 11 more
```

Each cycle re-sends the accumulated context.
Cached input is roughly a tenth the price of uncached input, so ten cached
cycles cost about one uncached request, but fourteen sequential cycles also cost
fourteen units of latency and fourteen turns of the agent loop.

The obvious fix is to give each tool a multi-argument schema.
That fix has two problems.

**It does not reach MCP tools.** `jp_mcp::server::resolve_tool` builds a
`ToolDefinition` uniformly for local, built-in, and MCP sources; for MCP it
takes the server's own `inputSchema` and applies configured overrides.
JP does not own those schemas and cannot edit them.
A per-tool fix helps only the tools JP wrote.

**It entangles two axes.** "What this tool does" and "how many operations fit in
one call" want to vary independently.
Hand-writing the second into every tool's schema means every new tool re-decides
it, and every existing tool that wants it needs a schema change, a validation
change, and a result-format change.

Fan-out makes the second axis a property of the tool *configuration* rather than
the tool *implementation*, so it applies uniformly to every source, including
MCP servers JP does not control.

## Design

### What the user sees

Nothing changes.
Fan-out exists on the wire and is invisible in the terminal.

A call carrying three operations renders as three tool calls, prompts for
permission three times unless the tool runs with `run = "allow"`, and shows
three results:

```text
Calling tool fs_read_file(path: "docs/pitch.md", start_line: 58, end_line: 68)
Calling tool fs_read_file(path: "scripts/migrate.sh", start_line: 1928, end_line: 1940)
Calling tool fs_read_file(path: "scripts/migrate.sh", start_line: 2072, end_line: 2082)
```

A call carrying one operation, in the envelope or bare, renders as one tool
call, indistinguishable from a call made without fan-out.

This invariant settles the permission question: whatever is rendered as a
distinct call is approved as a distinct call.
Collapsing three rendered calls into one approval prompt would ask the user to
approve something other than what they were shown.
The savings this RFD delivers are entirely in round-trips to the provider, not
in user interaction, and the tools that dominate the cost (`run = "allow"`
readers) prompt zero times either way.

### The provider boundary

Fan-out is a property of the tool as JP's MCP server advertises it, not of any
provider or client.

- Every caller learns a tool's shape from one place, `Service::definitions()`:
  JP's own provider request, MCP `tools/list` for Claude Code on the ACP route
  ([RFD 110]) or any other client, and the `describe_tools` built-in.
- Every call reaches the server the same way, whoever submits it, and the server
  expands it.

A provider sends the schema it is given, so `jp_llm` has no fan-out code, and
`jp_tool::ToolDefinition` has no fan-out field.
A route that bypassed the server for a fanned-out tool would be a bug in that
route, not a case this design has to handle.

### The envelope

When a tool opts in, the schema the server advertises is wrapped:

```json
{
  "type": "object",
  "properties": {
    "ops": {
      "type": "array",
      "minItems": 1,
      "description": "The operations to perform. Each element is one complete set of this tool's arguments.",
      "items": { "<the tool's own schema>": "..." }
    }
  },
  "required": ["ops"],
  "additionalProperties": false
}
```

`ops` rather than `arguments` or `args`: each element is one **operation**, and
the tool's existing parameter object is already "the arguments".

A `$defs` or `definitions` block moves from the tool's schema to the envelope's
root.
Same-document references are anchored at the root (`#/$defs/Name`), so leaving
the block nested under `properties.ops.items` would point every reference at a
root that no longer holds it.
MCP servers routinely declare such blocks, and they are the tools fan-out exists
to reach.
A schema that refers to its own root (`$ref: "#"`) is not rewritten.

The advertised description gains one sentence asking the assistant to batch
every operation it already knows it needs into one call.
It does not explain the tool's examples: each shows one bare operation, which is
itself a valid call (see below).

### Bare calls

A call to a fan-out tool whose arguments are not an envelope runs as an ordinary
call, exactly as if the tool had no `fan_out`: no child invocations, no framing.
What counts as an envelope depends on whether the tool's own schema declares a
top-level `ops` parameter.

**The tool does not declare `ops`** (the common case).
An `ops` key is always the envelope, and its absence is a bare call.
An envelope that is not a non-empty array of objects, or whose operation fails
validation, answers with an error naming the operation and what is wrong with
it.
Falling back to a bare call here would replace "operation 2: unknown argument
`pth`" with "unknown argument `ops`".

**The tool declares `ops`.** The arguments are an envelope only if every check
passes: `ops` is a non-empty array, every element is an object, and every
element validates against the tool's own schema, with coercion and defaults
applied to a copy.
If any check fails, the `ops` is the tool's own and the call runs bare.
When both readings validate (possible when the tool requires nothing, as in
`{"ops": [{"dry_run": true}]}`), the envelope wins: it is the shape every caller
was shown.

The advertised schema still requires `ops`.
Strict tool use on OpenAI and Anthropic rejects a schema whose root is an
`anyOf`, so a schema permitting both shapes at the root cannot be advertised to
them, and merging the tool's properties into the envelope as optional would drop
every `required` the tool declares.
Bare calls are accepted, not advertised: a model or client that sends one gets a
working call, and one that follows the schema batches.

### Where expansion happens

`Service::start_call` receives a call.
For a fan-out tool carrying an envelope it becomes a **parent invocation**, and
each operation becomes a **child invocation**:

- A child is an ordinary invocation with its own `InvocationId`, its own
  cancellation token (a child of the parent's), its own attempts, and its own
  stderr progress stream.
  It runs the same `run_call` state machine a plain call does, against the
  tool's own schema, so coercion, defaults, validation, the argument formatter,
  and questions work per operation unchanged.
- The parent runs no tool.
  It starts the children, applies the concurrency and error policy, waits for
  every child to settle, folds the results, and owns the one `Record` barrier
  and the one MCP response.

The server re-executes a tool after an answered question.
Because each operation is its own child, that re-runs only the child that asked;
operations that already finished are never recomputed.

Two Host-side mechanisms carry over without modification:

- **Inquiry ids do not collide.** `TurnState::next_inquiry_attempt` increments
  per `(tool_id, question_id)`, so two operations asking the same question under
  one tool call id get distinct attempts.
- **"Answer once for the whole call" already works.** Before prompting, the Host
  consults `remembered_tool_answers`, keyed by `(tool_name, question_id)`.
  An answer given at `PersistLevel::Turn` resolves every later operation
  silently, each still recorded as its own inquiry pair.

### Child invocations on the Host channel

A child talks to the Host through the same private channel and interactions as
any call, with three additions.

**Every interaction says which operation it is for.** `CallInfo` gains
`operation: Option<Operation>`, where `Operation` carries the parent's
`InvocationId`, the operation's zero-based `index`, and the operation `count`.
`CallInfo::request` stays the caller's original request, envelope included, so
the Host's correlation check sees the same request for every child.
The operation's own arguments arrive where they always do, in `Prepare` and
`Release`.

**A child ends with `Settled`, not `Record`.** The conversation records one
response per tool call, so a child is never recorded on its own.
Its last interaction is a new `Interaction::Settled`, carrying a `Recording` of
the operation: its executed arguments, its unedited result (absent if it never
executed), and the result approved for delivery.
The Host takes it and replies at once, so the parent can fold without waiting on
the terminal.

Every child the service concludes sends exactly one `Settled`, including one a
`stop` policy ruled out.
A child the Host resolves itself with `complete_call` (as it does for Stop &
respond) sends none, because the Host already holds its result.

**The parent's `Record` carries the operations.** `Recording` gains `operations:
Vec<Recording>`, one entry per child in the order the assistant wrote them.
`result` holds the folded body, which is also what the MCP caller receives, so
what JP records and what it delivers are the same text.

A plain call, including a bare call to a fan-out tool, has `operation: None` on
every interaction and an empty `operations` list, and sees no change.

### Scheduling

Every child prepares as soon as the parent starts it: it asks for argument
rendering and for admission immediately, so the user approves every operation of
a call before any of them runs, as with separate calls in one turn.

The policy sits between release and execution.
Once the Host releases a child, the child waits for a concurrency slot before
its tool runs.
Under a limit, children take slots in the order the assistant wrote them, so
`concurrency = 1` runs them in that order whatever order the Host released them
in.
Once a slot is free, the parent checks the error policy: under `on_error =
"stop"`, a child behind a failed operation settles as not run instead of
executing.

A child released to run reports its failure before it gives up its slot, so the
next child cannot slip through.
That covers a tool that ran and reported an error, and one that could not be run
at all (a command that fails to spawn, an upstream MCP error).
The tool's own result decides, before result-mode policy applies, so `result =
"skip"` or a declined `result = "ask"` review cannot hide the failure.
A child that ended without running (a Host decision, an argument the tool
rejects) reports a failure from the result it ended with.

The parent waits on its children, not on the Host, so a `stop` that rules out
every remaining operation ends the call instead of leaving it waiting for work
that will never start.

### What the Host does

The Host (`jp_cli::cmd::query::tool`) keeps its role: prompt, render, route
questions, review, record.
It neither schedules nor folds.

- The Host submits a fan-out call like any other tool call.
  It runs the same `split` the server does, only to open one executor per
  operation before the first interaction arrives, and refuses to route an
  operation the two sized differently.
- It demultiplexes child interactions by `operation.index` into one executor per
  operation.
  An operation's display state is keyed by `<tool call id>#<index>`, so each
  prompt, running state, and result belongs to its own line.
- Each operation is a call of its own in the coordinator's batch: announced,
  prompted for, and released in the order the assistant wrote it, like any call.
- `Settled` completes an operation's executor; the parent's `Record` completes
  the tool call and is what the conversation stores.
  The Host asks for that `Record` once every operation has a response, and
  counts the batch as settled only once it arrives.
- Every operation, whoever decided its outcome (the tool, the user at its prompt
  or result review, a cancellation), is resolved with `Executor::settle`, which
  answers whatever barrier the child is parked on.
  An operation cannot wait for the call's acknowledgement the way a plain call
  does, because the call is only recorded once every operation is resolved.
  Settling first makes sure the call's MCP request was sent: an operation
  settled before any was prepared would otherwise wait on a child the service
  never started.
- An operation waiting behind a concurrency limit is an executor whose release
  has not executed yet; the Host needs no queue of its own.

### Folding results

The parent folds its children in the order the assistant wrote them, not the
order they finished.

A fan-out call carrying one operation is recorded bare, success or failure, so
at N=1 the envelope leaves no trace in the result.
Otherwise the folded body frames each operation and states what did not run:

```text
[1/5] ok
File deleted.

[2/5] error
File has uncommitted changes. Please stage or discard first.

[3/5] not run (stopped after operation 2 failed)
[4/5] not run (stopped after operation 2 failed)
[5/5] not run (stopped after operation 2 failed)
```

Without the "not run" lines the assistant assumes all five were attempted and
reasons from a false premise.

The call is recorded as an error when no operation succeeded, such as one whose
first operation failed and stopped the rest.
One success is enough for it to count as a success; the framed sections name
the operations that failed.
The provider sees the same distinction it would for a bare call to the tool,
and replay picks the error or the success style by it.

### Interrupts

The Host addresses a fan-out call through its parent and each operation through
its child:

- **Stop & respond** holds every unfinished child and completes each with the
  cancellation response, running or waiting for a slot.
  The parent folds those like any other result, so the call records one response
  saying which operations were cancelled.
- **Restart** pauses the unfinished children.
  Settled children keep their results; the others re-prepare on resume, and the
  Host re-renders and re-prompts only those.
- **Escalating** past the menu cancels the Host's attempts; each unfinished
  child ends as cancelled and nothing further starts.

### Rendering and replay

Replay re-expands a recorded envelope for a tool whose turn configuration has
`fan_out`, using the same `expand` the server does, so `jp conversation print`
shows what was shown live.
A recorded call without an `ops` key renders as one call.
Replay has the configuration but not the tool's schema, so a bare call to a tool
that declares its own `ops` parameter replays as operations.

Custom-formatter output is stored on the `ToolCallRequest` event, which carries
the whole call.
Each operation's rendered chunk is joined with a newline into one string, in the
order the assistant wrote the operations, which keeps the stored value a string
for conversations recorded before fan-out existed.
An operation announced again after a restart replaces its chunk.
Replay therefore shows every operation's header and then every description,
where the live output put each description under its own header.

### Configuration

`fan_out` accepts a bool or a table, following the precedent `enable` already
sets in the same config tree:

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

`concurrency` is one integer rather than a `mode = "parallel" | "sequential"`
enum plus a later `max_concurrency`.
Two knobs that can contradict each other (`mode = "sequential", max_concurrency
= 4`) are one axis wearing two names.
A future `delay` key for rate-limited endpoints slots in beside these without
disturbing either.

`on_error` is genuinely independent of `concurrency`: independent reads want
unbounded concurrency and `continue`, ordered writes want `concurrency = 1` and
`stop`, and both other combinations are reachable.

With `concurrency` above 1, `stop` means no further operations are *started*.
In-flight operations finish; nothing is aborted.

The table form turns fan-out on without naming `enabled`; `enabled = false`
turns it off.
A `concurrency` of `0` reads as unbounded rather than "never run anything".
`fan_out` is a per-tool key with no counterpart on the `conversation.tools.*`
defaults block, so no tool inherits it: whether a tool's calls may be batched
depends on what the tool does.

### Where the safety boundary sits

An earlier version of this design gated fan-out on tools that never return
`NeedsInput`, on the grounds that a question arriving mid-batch leaves earlier
side effects committed.

That gate is unnecessary, and the property it was reaching for is better
expressed by the config above.
A tool configured with `concurrency = 1` and `on_error = "stop"` behaves exactly
as separate calls would: when operation 2 asks a question, operation 1 has
committed and operations 3 through 5 have not started.
That is the same state the assistant would have reached by issuing two calls.

The question is not "can this tool ask?" but "is this tool's fan-out ordered?",
and the tool's configuration answers it.

## Drawbacks

**A second way to do the same thing.** `fs_modify_file` already accepts many
targets in one call, and `bash` accepts many commands.
Fan-out does not replace those and does not subsume them (see Non-Goals), so the
project carries two mechanisms that both mean "more than one thing per call".

**The advertised schema is not the tool's schema.** `jp_tool::schema` states
that a tool's parameters are held exactly as the source declared them.
That stays true of `ToolDefinition`, but what the server advertises for a
fan-out tool is its own construction.

**The Host protocol grows.** `CallInfo::operation`, `Interaction::Settled`, and
`Recording::operations` are new, and the Host has to demultiplex one MCP call
into several executors.
Every Host implementation carries that, not only JP's terminal one.

**Replay loses operation boundaries in custom-formatter output.** The chunks are
joined into one string, so replay reproduces the printed text but cannot tell
which chunk belonged to which operation.

## Alternatives

**Per-tool multi-argument schemas.** Give `fs_read_file` a `reads[]` parameter,
`fs_create_file` a `files[]` parameter, and so on.
Rejected: it does not reach MCP tools, and it re-decides the same question in
every tool.
It also requires each tool to grow its own result-framing and partial-failure
handling, which fan-out provides once.

**Expansion in the Host.** Show the provider the envelope, and have the Host
expand a call into one MCP call per operation before submitting anything.
This was the first implementation.
Rejected: it depends on who submits calls.
When an agent submits its own calls, the Host never sees them before the server
does, so the agent can only be shown the tool's plain schema and fan-out is
silently off on that route.
It also records a folded body no MCP call delivered, and needs per-operation
acknowledgement to reconcile the two.

**One loop over operations inside a single invocation.** Rejected: the server
re-executes a tool after an answered question, so a loop inside one invocation
re-runs committed side effects, and one invocation carries one Host approval,
where fan-out needs one per operation.
Child invocations keep both per operation.

**Gate fan-out on tools that cannot ask questions.** Rejected in favour of the
`concurrency` and `on_error` configuration, which expresses the same safety
property without excluding every write tool from the feature.

## Non-Goals

**Replacing batch operations.** `fs_modify_file` is one operation over a set of
targets, not N independent operations: its patterns apply in order across files
and each sees the previous one's output, it shows a single diff for one
approval, and it validates the whole set before writing anything.
Tools shaped that way opt out by never setting `fan_out`.

**Shared parameters across operations.** Every operation carries its own
complete argument object.
Hoisting a common value to the call level (the way `fs_modify_file` has a
call-level `path` default) is a possible later extension and is not designed
here.

**A delay knob.** Rate-limited endpoints will want one.
The config shape above leaves room for it; this RFD does not build it.

**Changing what any tool does.** No tool implementation changes.
A tool that gains fan-out behaves identically per operation.

## Risks and Open Questions

**Does the assistant use it?** A wrapped schema is a more complex schema.
Some models may keep issuing one operation per call, which costs an extra
envelope of tokens per call and delivers nothing.
Enabling it on `fs_read_file` first and measuring the operation-count
distribution answers this before the feature spreads.

**Permission fatigue on write tools.** Fan-out plus a write tool that asks
before running means N prompts.
That is the honest behaviour, but it may make fan-out unattractive on exactly
the tools where ordering matters most.
The mitigation is the permission decision a user can remember for the rest of
the turn, which approves the remaining operations without prompting.
Whether that is enough is an observation to make in use.

**Do clients validate against the advertised schema?** An MCP client that checks
arguments against `inputSchema` before sending would refuse a bare call the
server accepts.
That costs nothing a client following the schema would miss, but it means bare
calls are a leniency of the server, not a promise to every caller.

## Implementation Plan

### Phase 1: Configuration

Add `FanOutConfig` to `jp_config::conversation::tool` with bool-or-table
parsing, `concurrency`, and `on_error`.
No behaviour yet.

**Depends on:** nothing.
**Mergeable:** yes.

### Phase 2: Advertisement

Add `jp_mcp::server::fan_out` with `envelope`, `split`, and `fold`, where
`split` tells an envelope from a bare call as described under Bare calls.
Have `Service::definitions()` advertise the envelope and the batching sentence,
and have JP's provider request take its tool definitions from the same place.
`jp_tool` and `jp_llm` carry no fan-out code.

**Depends on:** Phase 1.
**Mergeable:** no; a tool advertised with the envelope needs Phase 3 to run.

### Phase 3: Child invocations

Add `CallInfo::operation`, `Interaction::Settled`, and `Recording::operations`.
Expand an envelope into child invocations under a parent, apply `concurrency`
and `on_error` between release and execution, and fold into the parent's
`Record`.
Accept bare calls as plain calls.

**Depends on:** Phase 2.
**Mergeable:** with Phase 4.

### Phase 4: Host

Demultiplex child interactions into one executor per operation, key display
state by operation, complete operations on `Settled` and the tool call on the
parent's `Record`, and resolve every operation with `Executor::settle`.
Route pause, resume, and cancel to the right invocation.

**Depends on:** Phase 3.
**Mergeable:** with Phase 3.

### Phase 5: Enable and measure

Turn on `fan_out` for `fs_read_file`, `fs_grep_files`, and `fs_list_files`.
Record the distribution of operations per call over a week of use before
enabling it anywhere else.

**Depends on:** Phase 4.
**Mergeable:** yes.

## References

- [RFD 082] records every tool question round-trip as an inquiry pair, which is
  what keeps N per-operation questions individually auditable under one tool
  call id.
- [RFD 109] runs every tool call through JP's in-process MCP server, which is
  where fan-out lives.
- [RFD 110] lets an external agent submit tool calls itself; those calls reach
  the same server and fan out the same way.

[RFD 082]: 082-unified-inquiry-event-recording.md
[RFD 109]: 109-in-process-jp-mcp-server.md
[RFD 110]: 110-anthropic-subscription-queries-via-acp.md
