# RFD 113: Transparent Tool Call Fan-Out

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-18
- **Summary**: A per-tool `fan_out` setting lets one tool call carry several
  independent operations, which JP runs separately and folds into one response.

## Summary

This RFD adds **fan-out**: a per-tool configuration flag that lets a single tool
call carry several independent operations, which JP executes as if the assistant
had issued them separately.
The tool's own implementation is unchanged.
JP wraps the tool's parameter schema in an envelope before showing it to the
provider, expands the envelope into N executors before running anything, runs
each as its own call to JP's in-process MCP server, and folds the N results back
into one response.

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
permission three times if the tool is not unattended, and shows three results:

```text
Calling tool fs_read_file(path: "docs/pitch.md", start_line: 58, end_line: 68)
Calling tool fs_read_file(path: "scripts/migrate.sh", start_line: 1928, end_line: 1940)
Calling tool fs_read_file(path: "scripts/migrate.sh", start_line: 2072, end_line: 2082)
```

A call carrying one operation renders as one tool call, with the envelope
stripped, indistinguishable from a call made without fan-out.

This invariant settles the permission question: whatever is rendered as a
distinct call is approved as a distinct call.
Collapsing three rendered calls into one approval prompt would ask the user to
approve something other than what they were shown.
The savings this RFD delivers are entirely in round-trips to the provider, not
in user interaction, and the tools that dominate the cost (`run = "unattended"`
readers) prompt zero times either way.

### The envelope

When a tool opts in, the schema shown to the provider is wrapped:

```json
{
  "type": "object",
  "properties": {
    "ops": {
      "type": "array",
      "minItems": 1,
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

A tool's `examples` block needs no rewrite.
Each existing example shows exactly one operation, which is exactly the shape of
one `ops` element.
A generated sentence appended to the tool's description says so.

Both the envelope and the sentence are for the LLM provider only.
`ToolDefinition` keeps the tool's own schema and documentation, and
`provider_schema()` and `provider_description()` apply the wrap when a provider
builds its request.
Anything else that reads the definition, such as an MCP client listing JP's
tools or the `describe_tools` built-in, sees one operation's shape.

The envelope and its inverse live together in `jp_tool::fan_out`: `envelope`
builds the schema, `expand` takes a call's arguments apart into operations, and
`fold` frames the results.

### Where expansion happens

Expansion happens at the **executor plan**, before any tool runs.
One `ToolCallRequest` becomes N executors that share a tool call id and differ
only in their arguments.

This placement is the whole design.
The execution phase in `jp_cli::cmd::query::tool::coordinator` already keys its
per-call state by a flat local index: the executor, its accumulated answers, and
the review the Host settles on all live in `PhaseState` under that index.
Fan-out gives each operation an index of its own.
When a tool asks a question, the coordinator prompts and re-spawns **only that
index**; completed operations sit in `PhaseState::reviews` and are never
recomputed.

So per-operation resumption is free.
An operation that returns `NeedsInput` suspends alone.
Operations that already completed are not re-run.
Operations that have not started are unaffected.

Expanding inside a single executor instead would lose all of this: the
coordinator re-spawns an executor from scratch, so a batch loop inside one
executor would re-run its own committed side effects after every answered
question.

Two existing mechanisms carry over without modification:

- **Inquiry ids do not collide.** `TurnState::next_inquiry_attempt` increments
  per `(tool_id, question_id)`, so two operations asking the same question under
  one tool call id get distinct attempts.
  The counter was built for retries and covers fan-out unchanged.
- **"Answer once for the whole call" already works.** Before prompting, the
  coordinator consults `remembered_tool_answers`, keyed by `(tool_name,
  question_id)`.
  An answer given at `PersistLevel::Turn` resolves every later operation
  silently, each still recorded as its own inquiry pair.

### Executors and state keys

The coordinator takes the envelope apart, not the executor source, and asks the
source for one executor per operation:

```rust
// jp_cli::cmd::query::tool::executor
fn create(
    &self,
    request: ToolCallRequest,
    config: ToolConfigWithDefaults,
    op: Option<usize>,
) -> Option<Box<dyn Executor>>;

// jp_cli::cmd::query::tool::coordinator
pub fn prepare_one(&mut self, request: ToolCallRequest)
    -> Result<Vec<Box<dyn Executor>>, ToolCallResponse>;
```

`request.arguments` holds one operation's arguments, so a source never sees the
envelope.
`op` is the operation's position, or `None` for a call to a tool without
fan-out.
`create` returning `None` still means the tool could not be resolved and becomes
a "tool is not available" response.

A malformed envelope never reaches a source.
`expand` rejects a missing `ops` key, a non-array, an empty array, and a
non-object element, and `prepare_one` answers with a message naming which one it
was.
A zero-operation call is a model mistake, and an empty success would teach it
nothing.
A call that skips the envelope and sends one operation's arguments bare is
refused the same way rather than run: silently accepting it would teach the
model that the envelope is optional.

Operations share a tool call id, so display state cannot be keyed by it.
`Executor::state_key()` is the tool call id for a call without fan-out and
`<id>#<op>` for an operation, and `PermissionInfo` carries the same key.
Every prompt-state transition uses it.
Writing `AwaitingPermission` under one key and `Running` under the other would
strand the first entry, leave `is_prompting` true for the rest of the turn, and
decline every Ctrl-C as though a prompt were still open.

`ToolDefinition.parameters` holds the **per-operation** schema, so
`coerce_arguments`, `apply_parameter_defaults`, and `validate_tool_arguments` in
`jp_tool::definition` run against it unchanged.
`resolve_tool` validates that schema before it sets `ToolDefinition.fan_out`:
what a tool must declare is a property of the operation, and the envelope is
JP's construction.
Every provider module (Anthropic, Cerebras, Google, Ollama, OpenAI, OpenRouter,
and the OpenAI-compatible path shared by llama.cpp and vLLM) builds its request
from `provider_schema()` and `provider_description()`, and so does the context
window estimate in `jp_llm::window`.

### One MCP call per operation

JP runs every tool through its in-process MCP server ([RFD 109]), with the
query acting as the MCP Host.
Each operation is its own MCP call with its own correlation key, approved,
executed, and reviewed on its own.

The conversation records one folded response per tool call, but the server
delivers each call's own result, and acknowledging a call checks that what it
delivered matches the review it is acknowledged with.
The folded body is no single operation's result, so it cannot be that review.
The Host keeps both:

- `Review` carries an `op`, and the Host's call registry is keyed by `(tool call
  id, op)` so a review reaches the call it belongs to rather than a sibling.
- The coordinator records the folded review in the conversation and keeps each
  operation's review aside.
  `ToolCoordinator::acknowledge_reviews` replaces a fanned-out call's review
  with its operations' before acknowledging.
- An operation decided before it ran (skipped at the permission prompt) is
  acknowledged with that decision, and one that was never started (ruled out by
  `stop`, or cancelled while queued) with an error saying why.
  Either releases the barrier its prepared call is parked on.

### Agent-submitted calls

When an external agent submits tool calls itself ([RFD 110]), it learned JP's
tools from MCP `tools/list`, which carries each tool's own schema and never the
envelope.
Its arguments are one operation's, so the coordinator does not expand them: it
consults a tool's fan-out policy only while JP submits the calls
(`ToolExecution::Caller`).
Fan-out saves round-trips only where JP builds the provider request.

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

`stop` acts on what the tool did, not on what the assistant is told.
`result = "skip"` and a declined `result = "ask"` prompt both answer the
assistant with a success whatever the tool reported, and `skip` replaces the
result before the Host sees it.
The Host reads the tool's own outcome from the unedited result the server offers
for recording (`Executor::tool_failed`).
An operation that fails before the execution loop starts, such as one whose
argument formatter the server rejects, counts too.

When `stop` rules out every remaining operation before any of them started,
nothing is spawned and no event will ever arrive.
The execution loop checks whether every operation is accounted for before it
waits, not after handling an event, so such a call ends the phase instead of
hanging it.

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

### Folding results

N operations sharing one id become one recorded response.
`Schedule::fold` in `jp_cli::cmd::query::tool::schedule` does the folding, in
the order the assistant wrote the operations rather than the order they
finished.

A call to a tool without fan-out is recorded with its one operation's review
verbatim.
An error stays an error, and a Host edit stays an edit.
Framing it would turn a failure into a success carrying error text, which
reaches Anthropic as `is_error: false` and renders in the success style on
replay.
A fanned-out call carrying one successful operation is recorded bare, so at N=1
the envelope leaves no trace in the result.

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

A call whose every operation was decided before running (all skipped at the
permission prompt, or rejected while preparing) folds the same way without
entering the execution loop.

### Interrupts

Ctrl-C reaches the execution phase, and the phase applies the choice to every
operation it holds:

- **Stop & respond** cancels every unfinished operation, running or still queued
  behind a concurrency limit.
  Each answers with the cancellation response, framed with its position.
- **Restart** pauses the MCP calls of running and queued operations alike, so
  the re-run continues the same calls instead of submitting new ones.
- **Escalating** past the menu starts nothing further on the way out.

In every case the schedule stops handing out queued operations: they were never
spawned, so the loop would otherwise wait for results that cannot arrive.

### Rendering and replay

One call line and one permission prompt per operation, as described above.
Replay re-expands the recorded envelope when the turn's configuration gives the
tool `fan_out`, so `jp conversation print` shows what was shown live.

Custom-formatter output is stored on the `ToolCallRequest` event, which carries
the whole call.
Each operation's rendered chunk is joined with a newline into one string, which
reproduces on replay exactly what was printed live and keeps the stored value a
string for conversations recorded before fan-out existed.

## Drawbacks

**A second way to do the same thing.** `fs_modify_file` already accepts many
targets in one call, and `bash` accepts many commands.
Fan-out does not replace those and does not subsume them (see Non-Goals), so the
project carries two mechanisms that both mean "more than one thing per call".

**The schema is rewritten.** `jp_tool::schema` states that a tool's parameters
are held exactly as the source declared them, and that adapting a schema is the
provider's job.
Fan-out is the first exception, and the module doc names it.

**One tool call id no longer names one thing.** Display state is keyed by
operation, the Host's call registry by `(id, op)`, and acknowledgement by
operation, while the conversation still records one response per id.
The three have to agree, and each is a place a future change can key by id
alone and silently mis-route an operation.

**What is recorded differs from what is delivered.** The conversation stores the
folded body; each MCP call delivers its own operation's result.
For calls JP submits nobody else reads those deliveries, but the two are no
longer the same text.

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

**Expansion inside the execution service.** Loop over operations inside one
call to `jp_mcp::server`, which is already the single funnel for all three
sources.
Rejected: the server re-executes a tool after an answered question, so a loop
inside one call re-runs committed side effects, and one MCP call carries one
Host approval, where fan-out needs one per operation.
The executor plan is one layer up and already has the per-operation state this
needs.
The plan is also the stabler place to sit: expanding before execution means
fan-out is indifferent to how any single operation is dispatched.
Moving execution into the MCP server after this design was written confirmed
it: the expansion itself carried over unchanged.

**Fan-out for agent-submitted calls.** Advertise the envelope in `tools/list`
so an external agent can batch too.
Rejected for now: `tools/list` is also what third-party MCP clients read, and
they would all have to learn an envelope JP invented.

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

**Permission fatigue on write tools.** Fan-out plus a non-unattended write tool
means N prompts.
That is the honest behaviour, but it may make fan-out unattractive on exactly
the tools where ordering matters most.
The mitigation is the permission decision a user can remember for the rest of
the turn, which approves the remaining operations without prompting.
Whether that is enough is an observation to make in use.

## Implementation Plan

### Phase 1: Configuration

Add `FanOutConfig` to `jp_config::conversation::tool` with bool-or-table
parsing, `concurrency`, and `on_error`.
No behaviour yet.

**Depends on:** nothing.
**Mergeable:** yes.

### Phase 2: Schema envelope

Add `jp_tool::fan_out`, and the `fan_out` field, `provider_schema()`, and
`provider_description()` to `ToolDefinition`.
Set the field in `jp_mcp::server::resolve_tool`, and switch every provider
module and the context-window estimate to the accessors.
Behaviour-neutral while no tool opts in.

**Depends on:** Phase 1.
**Mergeable:** yes.

### Phase 3: Plan expansion

Add `op` to `ExecutorSource::create` and make `prepare_one` return one executor
per operation.
Key display state by `Executor::state_key()`.
Expand one request into N executors, only while JP submits the calls.
Run them under the existing unbounded model.

**Depends on:** Phase 2.
**Mergeable:** yes.

### Phase 4: Concurrency and error policy

Honour `concurrency` in the spawn loop and `on_error` in the event loop, reading
failures from the tool's own outcome.
Stop releasing queued operations on every interrupt that cancels the phase.

**Depends on:** Phase 3.
**Mergeable:** yes.

### Phase 5: Rendering and folding

Render one call line per operation, prompt per operation, fold N responses into
one with per-operation framing and "not run" lines.
Key the Host's call registry and `Review` by operation, and acknowledge each
operation's MCP call with its own review.
Join custom-formatter chunks into the existing string value.

**Depends on:** Phase 3.
**Mergeable:** yes, in parallel with Phase 4.

### Phase 6: Enable and measure

Turn on `fan_out` for `fs_read_file`, `fs_grep_files`, and `fs_list_files`.
Record the distribution of operations per call over a week of use before
enabling it anywhere else.

**Depends on:** Phases 4 and 5.
**Mergeable:** yes.

## References

- [RFD 082] records every tool question round-trip as an inquiry pair, which is
  what keeps N per-operation questions individually auditable under one tool
  call id.
- [RFD 109] runs every tool call through JP's in-process MCP server, which is
  why each operation is its own MCP call with its own acknowledgement.
- [RFD 110] lets an external agent submit tool calls itself; those calls are
  never fanned out.

[RFD 082]: 082-unified-inquiry-event-recording.md
[RFD 109]: 109-in-process-jp-mcp-server.md
[RFD 110]: 110-anthropic-subscription-queries-via-acp.md
