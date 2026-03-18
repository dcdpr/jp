# RFD 037: Await Tool for Stateful Handle Synchronization

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-08

## Summary

This RFD introduces an `await` built-in tool that allows the assistant to
synchronize on stateful tool handles from [RFD 009]. The assistant spawns tools
in the background, then calls `await` with handle IDs grouped by completion mode
(`any` and/or `all`). JP blocks the tool call until the condition is met and
returns the state of all referenced handles.

## Motivation

[RFD 009] introduces stateful tools — the assistant can spawn a tool, get a
handle ID back, and later fetch or apply input to that handle. But it has no
synchronization primitive. If the assistant spawns `cargo_check` and
`cargo_test` in parallel, it must poll each handle individually with `fetch` to
discover when they finish. This has two problems:

1. **Busy-waiting.** The assistant must guess when to poll. Poll too early and
   the tool is still running, wasting a tool call round-trip (and tokens). Poll
   too late and the assistant sits idle.

2. **No cross-tool coordination.** The assistant can't express "wait for both of
   these to finish" or "wait for whichever finishes first." It can only ask
   about one handle at a time, through each tool's own `fetch` action.

[RFD 009] explicitly lists "parallel stateful tools" as a non-goal and
identifies "proactive delivery of stopped handles" as an open question. The
recommended approach there is assistant-driven polling. This RFD replaces
polling with a purpose-built synchronization tool.

Without `await`, the assistant's only options are:

- **Poll in a loop.** Each `fetch` is a full LLM round-trip. A 5-second build
  might need 10 fetch calls before it catches the completion.
- **Guess and hope.** The assistant does other work and checks later, risking
  stale results or missed errors.
- **Give up on parallelism.** Run tools sequentially, which defeats the purpose
  of stateful handles.

`await` gives the assistant an explicit, efficient synchronization point.

### Concrete example

The assistant runs `cargo check` and `cargo test` concurrently, then acts on the
combined results — all in a single tool call batch:

```txt
A: [
  call(cargo_check, { action: "spawn", id: "check", package: "jp_cli" })
    → { "id": "check", "state": "running" }

  call(cargo_test, { action: "spawn", id: "test", package: "jp_cli" })
    → { "id": "test", "state": "running" }

  call(await, { all: ["check", "test"] })
    → {
        "completed": [
          { "id": "check", "state": "stopped", "result": "ok, 0 warnings" },
          { "id": "test", "state": "stopped", "result": "test result: ok. 42 passed" }
        ],
        "pending": []
      }
]
```

Because the assistant chooses the handle IDs (see [Model-chosen handle
IDs](#model-chosen-handle-ids)), it can reference them in the `await` call
within the same batch. No extra round-trip.

Or a race pattern — try two search approaches, take whichever returns first:

```txt
A: [
  call(crate_search, { action: "spawn", id: "crates", query: "async runtime" })
    → { "id": "crates", "state": "running" }

  call(github_code_search, { action: "spawn", id: "github", query: "async runtime" })
    → { "id": "github", "state": "running" }

  call(await, { any: ["crates", "github"] })
    → {
        "completed": [
          { "id": "crates", "state": "stopped", "result": "tokio, async-std, ..." }
        ],
        "pending": [
          { "id": "github", "state": "running" }
        ]
      }
]
```

## Design

### The `await` tool

`await` is a built-in tool, like `describe_tools`. It is not part of any
individual tool's action schema — it operates on the handle registry across all
stateful tools.

#### Schema

```json
{
  "name": "await",
  "description": "Block until stateful tool handles reach completion. Use `any` to wait for the first handle to finish, `all` to wait for every handle to finish, or both to combine conditions.",
  "parameters": {
    "type": "object",
    "properties": {
      "any": {
        "type": "array",
        "items": {
          "type": "string"
        },
        "description": "Handle IDs. Returns when at least one of these handles reaches 'stopped'."
      },
      "all": {
        "type": "array",
        "items": {
          "type": "string"
        },
        "description": "Handle IDs. Returns when every one of these handles reaches 'stopped'."
      },
      "timeout_secs": {
        "type": "integer",
        "description": "Maximum seconds to wait. If exceeded, returns with current handle states. Omit for no timeout."
      }
    },
    "anyOf": [
      {
        "required": [
          "any"
        ]
      },
      {
        "required": [
          "all"
        ]
      }
    ]
  }
}
```

At least one of `any` or `all` must be provided. Both may be provided
simultaneously.

#### Completion condition

`await` blocks until:

- Every handle in `all` has reached `Stopped`, **AND**
- At least one handle in `any` has reached `Stopped` (if `any` is provided)

If only `any` is provided, the `all` condition is trivially satisfied. If only
`all` is provided, the `any` condition is trivially satisfied.

A handle that is already `Stopped` when `await` is called counts immediately
toward the condition. If the condition is already met at call time, `await`
returns without blocking.

#### Response format

```json
{
  "completed": [
    {
      "id": "check",
      "state": "stopped",
      "result": "ok, 0 warnings"
    },
    {
      "id": "test",
      "state": "stopped",
      "result": "test result: ok. 42 passed"
    }
  ],
  "pending": [
    {
      "id": "github",
      "state": "running"
    }
  ]
}
```

`completed` contains all handles that have reached `Stopped` at the time of
return. `pending` contains handles still in `Running` or `Waiting` state. Both
arrays include handles from `any` and `all` — the grouping is by state, not by
which parameter they came from.

For stopped handles, `result` contains the tool's output (success) or error
message (failure). The assistant inspects the result to determine success or
failure, same as with a regular `ToolCallResponse`.

For pending handles (only present in `any` mode when the condition was met by a
subset), the response includes the current state so the assistant can decide
whether to await again, fetch individually, or abort.

#### Timeout behavior

When `timeout_secs` is specified and the timeout expires before the completion
condition is met, `await` returns with whatever states are current. The
`completed` array contains any handles that did finish; `pending` contains the
rest. The response includes a `"timed_out": true` field so the assistant can
distinguish a timeout from a normal return.

```json
{
  "completed": [
    {
      "id": "check",
      "state": "stopped",
      "result": "ok"
    }
  ],
  "pending": [
    {
      "id": "test",
      "state": "running"
    }
  ],
  "timed_out": true
}
```

No timeout is the default. Handles are not aborted on timeout — they continue
running and can be awaited again or fetched individually.

#### Error cases

| Condition                         | Behavior                                 |
|-----------------------------------|------------------------------------------|
| Both `any` and `all` are empty or | Error: "At least one handle ID required" |
| missing                           |                                          |
| Handle ID not found in registry   | Error: "Handle `my_handle` not found"    |
| Handle is in `Waiting` state      | Counts as pending. JP continues handling |
|                                   | the inquiry/prompt while `await` waits.  |
|                                   | The handle transitions to `Stopped` once |
|                                   | the question is answered and the tool    |
|                                   | finishes.                                |
| All handles already stopped       | Immediate return, no blocking.           |

An unknown handle ID is an error, not a silent skip. This catches typos and
stale IDs early.

### Integration with the handle registry

`await` operates on the `HandleRegistry` from RFD 009. When called, it:

1. Validates all referenced handle IDs exist in the registry.
2. Checks if the completion condition is already met. If so, returns
   immediately.
3. Subscribes to state-change notifications from the referenced handles.
4. Blocks (async) until the condition is met or the timeout expires.
5. Collects current states from the registry and returns.

Step 3 requires the handle registry to support notification — when a handle
transitions to `Stopped`, waiters are notified. This is a natural extension: the
registry already tracks handle state, and adding a `tokio::sync::Notify` or
channel per handle is straightforward.

```rust
struct HandleEntry {
    handle: ToolHandle,
    /// Notified when the handle reaches a terminal state.
    notify: Arc<Notify>,
}
```

Multiple `await` calls can reference the same handle concurrently. `Notify`
supports multiple waiters natively.

### Extending `BuiltinTool` with an execution context

The current `BuiltinTool` trait is stateless:

```rust
pub trait BuiltinTool: Send + Sync {
    async fn execute(&self, arguments: &Value, answers: &IndexMap<String, Value>) -> Outcome;
}
```

`await` needs per-execution state that this signature can't provide:

1. **Handle registry access** — to look up handles and subscribe to state change
   notifications.
2. **Cancellation token** — to abort on turn end or user interrupt.

The cancellation token already exists at the `ToolExecutor.execute()` level but
is never threaded down to `execute_builtin`. The handle registry (from [RFD
009]) would be shared state managed by the coordinator.

Three approaches were considered:

**Constructor injection (no trait change).** The `AwaitTool` struct holds an
`Arc<HandleRegistry>` injected at construction time. This works for the registry
but not the cancellation token, which changes per-execution. The trait has no
way to receive per-call state, so cancellation would require a workaround (e.g.,
the coordinator pre-setting a token on the struct before each call). Fragile.

**Coordinator interception (no trait change).** The coordinator checks for
`tool_name == "await"` and handles it directly, bypassing the builtin trait
entirely. Simplest to implement but doesn't generalize. Every future stateful
builtin would need its own special case in the coordinator.

**Execution context parameter (trait change).** Add a `BuiltinContext` struct to
the `execute` method:

```rust
pub struct BuiltinContext {
    pub cancellation_token: CancellationToken,
    pub handle_registry: Option<Arc<HandleRegistry>>,
}

#[async_trait]
pub trait BuiltinTool: Send + Sync {
    async fn execute(
        &self,
        arguments: &Value,
        answers: &IndexMap<String, Value>,
        ctx: &BuiltinContext,
    ) -> Outcome;
}
```

**This RFD recommends the execution context approach.** The ripple effects are
small:

- `DescribeTools::execute` adds an unused `_ctx: &BuiltinContext` parameter.
- `ToolDefinition::execute_builtin` constructs a `BuiltinContext` from state
  already in scope (the cancellation token is passed to `execute_local` and
  `execute_mcp` but currently dropped for builtins).
- `BuiltinContext` is defined in `jp_llm::tool::builtin`, alongside the trait.

The context struct starts small and grows as future builtins need more
capabilities (sub-agents, conversation state, etc.). The trait is internal and
not public, so the breaking change is confined to our codebase.

### Model-chosen handle IDs

In [RFD 009]'s original design, JP assigns handle IDs (`h_1`, `h_2`) and returns
them to the assistant. This forces a round-trip: spawn in one batch, get IDs
back, then use them in the next batch.

This RFD requires the assistant to choose handle IDs via a required `id`
parameter on the `spawn` action. This is a change to RFD 009's spawn schema:

```json
{
  "properties": {
    "action": {
      "const": "spawn"
    },
    "id": {
      "type": "string",
      "description": "Handle ID for this tool instance. Must be unique across active handles."
    }
  },
  "required": [
    "action",
    "id"
  ]
}
```

Because the assistant chooses the ID, it can reference handles in the same
batch:

```txt
A: [
  call(cargo_check, { action: "spawn", id: "check" }),
  call(cargo_test,  { action: "spawn", id: "test" }),
  call(await, { all: ["check", "test"] })
]
```

All three tool calls execute concurrently. The spawns register their handles;
the `await` blocks until both reach `Stopped`. Zero extra round-trips.

JP validates that the chosen ID doesn't collide with an existing active handle.
If it does, the spawn returns an error. The assistant picks descriptive names
(`check`, `test`, `build_release`) rather than opaque tokens — these appear in
the conversation history and should be readable.

Most LLM providers assign tool call IDs at the API level (not model-chosen), so
using provider-assigned IDs as handle IDs is not feasible. Model-chosen IDs
sidestep this entirely.

### `await` in parallel tool call batches

LLMs can request multiple tool calls in a single response. The `ToolCoordinator`
already runs these in parallel. The spawn+await-in-one-batch pattern works
because the coordinator pre-registers handle IDs before dispatching any tool in
the batch.

When the coordinator receives a batch of tool calls, it:

1. Scans the batch for `spawn` actions.
2. Pre-registers each spawn's `id` in the handle registry as a placeholder entry
   (state: `Pending`, no running process yet).
3. Dispatches all tool calls concurrently.

This guarantees that when `await` looks up a handle ID, the entry exists — even
if the corresponding `spawn` hasn't started executing yet. The `await` tool
subscribes to the handle's `Notify` and blocks until it transitions through
`Running` to `Stopped`.

Pre-registration also catches ID collisions early: if two spawns in the same
batch use the same `id`, or if a spawn's `id` collides with an already-active
handle from a previous batch, the coordinator rejects the batch before any tool
runs.

### Schema availability

`await` is always included in the tool list when the stateful tool protocol is
active (i.e., when at least one stateful tool is configured). If no stateful
tools are available, `await` is not exposed — there's nothing to await.

This mirrors how `describe_tools` is conditionally included based on whether
tool documentation exists.

## Drawbacks

**Token cost of a blocking tool call.** Each `await` is a full LLM tool call
round-trip. In the simple case where the assistant spawns two tools and
immediately awaits them, the `await` call adds one extra round-trip compared to
JP running the tools in parallel internally (which the current one-shot model
already does). The benefit only materializes when the assistant does useful work
between spawn and await.

**Complexity in the coordinator.** Adding a third dispatch path (`await` /
stateful / one-shot) increases the coordinator's branching. The coordinator is
already the most complex module in the query pipeline.

**LLM comprehension.** The assistant must understand the spawn → await pattern
and use it correctly. Current LLMs handle simple tool calls well, but multi-step
async patterns require more sophisticated planning. System prompt instructions
will help, but some models may struggle.

## Alternatives

### Rely on the system message queue (RFD 011)

Instead of `await`, let the system message queue notify the assistant when
handles finish. The assistant spawns tools and continues; JP delivers "tool
stopped" notifications piggybacked on the next message.

**Not sufficient because:** The system message queue is fire-and-forget — the
assistant can't choose when or how to synchronize. It also can't express "wait
for all of these" or "wait for any of these." The message queue is
complementary: it handles cases where the assistant forgets to await or where
delivery timing isn't critical. `await` handles cases where the assistant
explicitly wants to synchronize.

### Extend `fetch` to accept multiple handle IDs

Instead of a new tool, extend each stateful tool's `fetch` action to accept an
array of IDs and block until completion.

**Rejected because:** `fetch` is per-tool — it's part of each tool's action
schema. `await` is cross-tool: it synchronizes handles from different tools.
Making `fetch` cross-tool would break the per-tool schema model from RFD 009.

### Implicit parallelism — JP runs all spawned tools and collects results automatically

Instead of explicit spawn/await, JP detects independent tool calls and
parallelizes them internally, returning all results at once.

**Already exists for one-shot tools.** The `ToolCoordinator` runs all tool calls
in a batch concurrently. The stateful protocol exists for cases where implicit
parallelism isn't enough: long-running tools, interactive sessions, and cases
where the assistant wants to interleave work between spawn and result
collection.

### `select` / `race` as a separate tool alongside `await_all`

Split into two tools: `await_all` and `await_any` (or `select`).

**Rejected because:** A single `await` tool with `any`/`all` parameters is
simpler for the assistant and avoids schema proliferation. The combined form
also supports the (admittedly niche) case of waiting for a mix of conditions.

### Spawn configuration

This RFD introduces per-tool configuration for stateful handle behavior, nested
under `[conversation.tools.<name>.spawn]`:

```toml
[conversation.tools.cargo_check.spawn]
stateful = true

# Notification policy: when should JP notify the assistant about this handle?
[conversation.tools.cargo_check.spawn.notifications]
on_success = true # handle stopped with Ok result
on_failure = true # handle stopped with Err result
on_waiting = true # handle entered Waiting state (needs input)
on_content = false # new output while Running (noisy for builds)

# Lifecycle policy: what happens if this handle is still running when the
# turn would end?
[conversation.tools.cargo_check.spawn.lifecycle]
on_turn_end = "inquire" # "inquire" | "await" | "abort"
```

#### Notifications

Notifications are delivered via the system message queue ([RFD 011]). Each flag
controls whether a specific state change triggers a notification:

| Flag         | Triggers when                            | Default |
|--------------|------------------------------------------|---------|
| `on_success` | Handle reaches `Stopped` with `Ok`       | `true`  |
|              | result                                   |         |
| `on_failure` | Handle reaches `Stopped` with `Err`      | `true`  |
|              | result                                   |         |
| `on_waiting` | Handle enters `Waiting` (needs input)    | `true`  |
| `on_content` | Handle produces new output while         | `false` |
|              | `Running`                                |         |

`on_content` is `false` by default because it can be very noisy — every line of
build output would trigger a notification. It’s useful for tools where
incremental output matters (test runners, log tailers) but not for most build
tools.

Notifications only apply to handles that the assistant hasn’t explicitly polled
(`fetch`) or awaited. If the assistant is already watching a handle,
notifications for that handle are suppressed.

#### Turn-end behavior

[RFD 009] aborts all outstanding handles when a turn ends. This RFD extends that
with a configurable `on_turn_end` policy:

**`inquire`** (default): JP sends an `InquiryRequest` to the assistant with a
dynamically built schema listing each outstanding handle, its current state, and
trimmed output. The assistant chooses per-handle whether to wait or abort:

```json
{
  "handles": [
    {
      "id": "check",
      "tool": "cargo_check",
      "state": "running",
      "elapsed_secs": 3.2,
      "output_preview": "Compiling jp_cli v0.1.0...",
      "action": "wait | abort"
    },
    {
      "id": "test",
      "tool": "cargo_test",
      "state": "running",
      "elapsed_secs": 5.1,
      "output_preview": "running 42 tests...",
      "action": "wait | abort"
    }
  ]
}
```

The inquiry target is configurable via `AssistantOverrideConfig` (same pattern
as [RFD 034]), allowing the turn-end inquiry to be routed to a cheaper model
since it’s a simple classification task.

If the assistant chooses to wait, JP blocks until the handle reaches `Stopped`
(subject to a configurable timeout), then delivers the result as a
`ToolCallResponse` and lets the assistant respond again. If the assistant
chooses to abort, JP terminates the handle immediately.

**`await`**: JP automatically waits for all outstanding handles up to a
configurable timeout. No LLM round-trip. Results are delivered via the system
message queue at the start of the next turn. If the timeout is exceeded,
remaining handles are aborted.

**`abort`**: JP terminates all outstanding handles immediately. Results are
lost. This is the simplest option and appropriate for tools where partial
results have no value. This is the current behavior from RFD 009.

[RFD 034]: 034-inquiry-specific-assistant-configuration.md

## Non-Goals

- **Inter-handle communication.** Piping output from one handle to another
  (e.g., feeding `cargo check` output into a formatter). This is a different
  kind of coordination.
- **Automatic abort on `any` completion.** When an `any` condition is met, the
  remaining handles continue running. The assistant can abort them explicitly if
  desired. Auto-abort would be surprising.
- **Priority or ordering.** All handles are treated equally. No mechanism for
  "prefer this handle over that one if both complete simultaneously."
- **Recursive await.** The `await` tool itself is not a stateful tool — it
  cannot be spawned and awaited. It runs synchronously within the tool call
  batch (blocking from the coordinator's perspective, async internally).

## Risks and Open Questions

### Handle ID collisions

The assistant chooses handle IDs, so collisions are possible — either within a
batch (two spawns with the same `id`) or across batches (reusing an ID from a
still-active handle). Pre-registration catches both cases before any tool in the
batch executes. The batch is rejected with an error identifying the conflicting
ID, and the assistant must retry with a different name.

### Provider support for the `anyOf` required constraint

The schema uses `anyOf` on the `required` field to express "at least one of
`any` or `all` must be present." Some providers may not support this. Fallback:
make both optional in the schema and validate at runtime, returning a clear
error if neither is provided.

### `Waiting` handles and the inquiry system

A handle in `Waiting` state has a pending question — either routed to the user
(interactive prompt) or to the inquiry system (secondary LLM call). The `await`
tool does not treat `Waiting` as a completion condition. It continues blocking
while JP's existing machinery resolves the question:

- **User-targeted questions:** The prompt appears in the terminal. The user
  answers. The tool continues and eventually reaches `Stopped`.
- **Assistant-targeted questions:** The inquiry backend makes a structured LLM
  call (per [RFD 028]). The answer is delivered, the tool continues.

From `await`'s perspective, `Waiting` is just another non-terminal state, like
`Running`. The handle will transition to `Stopped` once the question is resolved
and the tool completes. If the inquiry itself fails or the user cancels the
prompt, the tool still reaches `Stopped` (with an error result).

The only risk is a question that blocks indefinitely (e.g., the user walks away
from a prompt). The `timeout_secs` parameter covers this case.

[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md

### Interaction with tool permission prompts

If the assistant spawns a tool that requires permission (`RunMode::Ask`) and
immediately awaits it, the permission prompt blocks the spawn. The `await` call
sees the handle in a pre-running state. This is fine — the handle doesn't reach
`Stopped` until the user approves and the tool completes. But it could be
confusing if the user takes a long time to approve.

## Implementation Plan

### Phase 0: `BuiltinContext` and trait change

Add `BuiltinContext` to `jp_llm::tool::builtin` with a single field:
`cancellation_token: CancellationToken`. Change `BuiltinTool::execute` to accept
`&BuiltinContext`. Update `DescribeTools` to accept and ignore the new
parameter. Thread the cancellation token from `ToolDefinition::execute` through
`execute_builtin` into the context.

This is a standalone improvement — builtins currently can't be cancelled, and
the token is already available one call frame up. No dependency on [RFD 009] or
the handle registry. The `handle_registry` field is added in Phase 1.

Can be merged independently. No behavioral change beyond making builtin
cancellation possible.

### Phase 1: Handle notification infrastructure

Extend the `HandleRegistry` (from [RFD 009] Phase 2) with per-handle `Notify`
channels. When a handle transitions to `Stopped`, all waiters are notified. Add
`handle_registry: Option<Arc<HandleRegistry>>` to `BuiltinContext`.

Can be merged independently. No behavioral change — notifications are emitted
but nothing listens yet.

Depends on: Phase 0, RFD 009 Phase 2 (handle registry exists).

### Phase 2: `await` dispatch in the coordinator

Add the `await` interception path to the `ToolCoordinator`. Implement the
blocking logic: validate handle IDs, check completion condition, subscribe to
notifications, block until condition met or timeout.

Return the response as a formatted `ToolCallResponse` with the JSON structure
described above.

Depends on: Phase 1, RFD 009 Phase 4 (stateful tool dispatch).

### Phase 3: Schema and conditional exposure

Register `await` in the tool list when stateful tools are active. Generate the
schema. Add system prompt guidance for the spawn/await pattern.

Depends on: Phase 2.

### Phase 4: Same-batch spawn + await

Implement pre-registration of handle IDs in the coordinator: scan each tool call
batch for `spawn` actions, register placeholder entries in the handle registry
before dispatching any tool. This guarantees `await` calls in the same batch
always find their handles.

Depends on: Phase 2. Can be merged independently.

## References

- [RFD 009: Stateful Tool Protocol][RFD 009] — the handle registry and stateful
  tool model that `await` builds on.
- [RFD 011: System Notification Queue][RFD 011] — complementary notification
  mechanism for handles that finish without being awaited.
- [Query Stream Pipeline](../architecture/query-stream-pipeline.md) — the turn
  loop and tool coordinator where `await` is dispatched.

[RFD 009]: 009-stateful-tool-protocol.md
[RFD 011]: 011-system-notification-queue.md
