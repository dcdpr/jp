# RFD 108: Transparent Tool Call Fan-Out

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-18

## Summary

This RFD adds **fan-out**: a per-tool configuration flag that lets a single tool
call carry several independent operations, which JP executes as if the assistant
had issued them separately.
The tool's own implementation is unchanged.
JP wraps the tool's parameter schema in an envelope before showing it to the
provider, expands the envelope into N executors before running anything, and
folds the N results back into one response.

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

**It does not reach MCP tools.** `resolve_tool` (`tool.rs:1299`) builds a
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
    "ops": { "type": "array", "items": { "<the tool's own schema>": "..." } }
  },
  "required": ["ops"]
}
```

`ops` rather than `arguments` or `args`: each element is one **operation**, and
the tool's existing parameter object is already "the arguments".

A tool's `examples` block needs no rewrite.
Each existing example shows exactly one operation, which is exactly the shape of
one `ops` element.
The schema wrap appends a generated sentence to the tool's description saying
so.

### Where expansion happens

Expansion happens at the **executor plan**, before any tool runs.
One `ToolCallRequest` becomes N executors that share a tool call id and differ
only in their arguments.

This placement is the whole design.
The coordinator already keys its per-tool state by execution index:
`executing_tools` (`coordinator.rs:989`), `accumulated_answers`
(`coordinator.rs:268`), and `results` (`coordinator.rs:990`).
When a tool asks a question, the coordinator prompts and re-spawns **only that
index** (`coordinator.rs:1811-1815`); completed executions sit in `results` and
are never recomputed.

So per-operation resumption is free.
An operation that returns `NeedsInput` suspends alone.
Operations that already completed are not re-run.
Operations that have not started are unaffected.

Expanding inside a single executor instead would lose all of this: the
coordinator re-spawns an executor from scratch, so a batch loop inside one
executor would re-run its own committed side effects after every answered
question.

Two existing mechanisms carry over without modification:

- **Inquiry ids do not collide.** `next_inquiry_attempt` (`coordinator.rs:1625`)
  increments per `(tool_id, question_id)`, so two operations asking the same
  question under one tool call id get distinct attempts.
  The counter was built for retries and covers fan-out unchanged.
- **"Answer once for the whole call" already works.** Before prompting, the
  coordinator consults `remembered_tool_answers`, keyed by `(tool_name,
  question_id)` (`coordinator.rs:1648-1666`).
  An answer given at `PersistLevel::Turn` (`coordinator.rs:1804-1809`) resolves
  every later operation silently, each still recorded as its own inquiry pair.

### Signature changes

`ExecutorSource::create` (`executor.rs:99`) returns one executor or `None`,
where `None` means the tool could not be resolved and becomes a "tool is not
available" response (`coordinator.rs:739-749`).

A bare `Vec` would make that indistinguishable from an empty `ops` array, so the
option stays:

```rust
// jp_llm::tool::executor
fn create(&self, request: ToolCallRequest, config: ToolConfigWithDefaults)
    -> Option<Vec<Box<dyn Executor>>>;

// jp_cli::cmd::query::tool::coordinator
pub fn prepare_one(&mut self, request: ToolCallRequest)
    -> Result<Vec<Box<dyn Executor>>, ToolCallResponse>;
```

`None` means the tool does not exist.
`Some(vec![])` means the tool exists and the assistant sent zero operations,
which is an error response rather than an empty success: a zero-operation call
is a model mistake, and returning nothing teaches it nothing.

`ToolDefinition.parameters` keeps holding the **per-operation** schema, so
`coerce_arguments_to_schema` (`tool.rs:733`), `apply_parameter_defaults`
(`tool.rs:870`), and `validate_tool_arguments` (`tool.rs:872`) run against it
unchanged.
A new `provider_schema()` accessor applies the wrap, and the seven provider
modules that read `tool.parameters` when building their request bodies switch to
it.

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

N `ToolCallResponse` values sharing one id become one.
`ExecutionResult.responses` already carries `(index, response)` pairs and
documents that merging back into stream order is the caller's job
(`coordinator.rs:182-186`), so the shape fits.

The folded body frames each operation and states what did not run:

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

## Drawbacks

**A second way to do the same thing.** `fs_modify_file` already accepts many
targets in one call, and `bash` accepts many commands.
Fan-out does not replace those and does not subsume them (see Non-Goals), so the
project carries two mechanisms that both mean "more than one thing per call".

**The schema is rewritten.** `json_schema` states that a tool's parameters are
held exactly as the source declared them, and that adapting a schema is the
provider's job.
Fan-out is the first exception.
The module doc has to name it, or the next contributor will read the invariant
and be wrong.

**Rendered-argument replay needs a new shape.** `RENDERED_ARGUMENTS_KEY`
(`event.rs:35`) stores one base64 blob per event so replay reproduces
custom-formatter output without re-running the formatter.
One event now carries N rendered chunks, so the value becomes a list.

**More state keyed by something other than tool id.** `tool_states`
(`coordinator.rs:381`) is a `HashMap<String, ToolCallState>` keyed by tool call
id.
N operations under one id need a composite key.

## Alternatives

**Per-tool multi-argument schemas.** Give `fs_read_file` a `reads[]` parameter,
`fs_create_file` a `files[]` parameter, and so on.
Rejected: it does not reach MCP tools, and it re-decides the same question in
every tool.
It also requires each tool to grow its own result-framing and partial-failure
handling, which fan-out provides once.

**Expansion inside a single executor.** Loop over operations in
`ToolDefinition::execute` (`tool.rs:793`), which is already the single funnel
for all three sources.
Rejected: the coordinator re-spawns an executor from scratch after an answered
question, so a loop inside one executor re-runs committed side effects.
The executor plan is one layer up and already has the per-operation state this
needs.
The plan is also the stabler place to sit: expanding before execution means
fan-out is indifferent to how any single operation is dispatched, so reworking
that dispatch neither blocks this design nor is blocked by it.

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

**Is `ops` the right name?** The original proposal spelled it `args`.
`ops` is used here because "operation" is the concept and "arguments" is already
what the inner object is, but the name lands in every fanned-out call the
assistant writes and is worth one round of review.

**Does the assistant use it?** A wrapped schema is a more complex schema.
Some models may keep issuing one operation per call, which costs an extra
envelope of tokens per call and delivers nothing.
Enabling it on `fs_read_file` first and measuring the operation-count
distribution answers this before the feature spreads.

**Permission fatigue on write tools.** Fan-out plus a non-unattended write tool
means N prompts.
That is the honest behaviour, but it may make fan-out unattractive on exactly
the tools where ordering matters most.
The existing `PersistLevel::Turn` answer cache is the mitigation; whether it is
enough is an implementation-time observation.

**Interrupt semantics.** Ctrl-C during a fanned-out call currently reaches an
execution phase, not an individual tool.
Whether "Stop & respond" should cancel the whole call or only the in-flight
operations needs a decision during implementation.

## Implementation Plan

### Phase 1: Configuration

Add `FanOutConfig` to `jp_config::conversation::tool` with bool-or-table
parsing, `concurrency`, and `on_error`.
No behaviour yet.

**Depends on:** nothing.
**Mergeable:** yes.

### Phase 2: Schema envelope

Add the fan-out field and `provider_schema()` to `ToolDefinition`.
Apply the wrap in `resolve_tool`, append the generated description sentence, and
switch the seven provider modules to the new accessor.
Behaviour-neutral while no tool opts in.

**Depends on:** Phase 1.
**Mergeable:** yes.

### Phase 3: Plan expansion

Change `ExecutorSource::create` and `prepare_one` to the plural signatures.
Give `tool_states` a composite key.
Expand one request into N executors.
Run them under the existing unbounded model.

**Depends on:** Phase 2.
**Mergeable:** yes.

### Phase 4: Concurrency and error policy

Honour `concurrency` in the spawn loop and `on_error` in the event loop.

**Depends on:** Phase 3.
**Mergeable:** yes.

### Phase 5: Rendering and folding

Render one call line per operation, prompt per operation, fold N responses into
one with per-operation framing and "not run" lines.
Change `RENDERED_ARGUMENTS_KEY` to hold a list.

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

[RFD 082]: 082-unified-inquiry-event-recording.md
