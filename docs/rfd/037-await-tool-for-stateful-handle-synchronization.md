# RFD 037: Await Tool for Stateful Handle Synchronization

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-08
- **Requires**: [RFD 009], [RFD 011]
- **Summary**: Introduces a built-in `await` tool that synchronizes on parallel
  stateful tool handles with `any` and `all` completion modes.

## Summary

This RFD introduces a built-in `await` tool for synchronizing on stateful tool
handles ([RFD 009]).
After spawning tools in the background, the assistant calls `await` with handle
IDs grouped by completion mode (`any`, `all`, or both).
JP blocks until the completion condition is met and returns the state of every
referenced handle.

## Motivation

[RFD 009] introduces stateful tools: the assistant can spawn a tool, receive a
handle ID, and later `fetch` output or `apply` input to that handle.
However, [RFD 009] leaves "parallel stateful tools" as a non-goal and "proactive
delivery of stopped handles" as an open question, relying instead on
assistant-driven polling.

When the assistant spawns `cargo_check` and `cargo_test` in parallel, polling
each handle individually with `fetch` has two problems:

1. **Busy-waiting.** Each `fetch` costs a full LLM round-trip and tokens.
   A 5-second build might take 10 `fetch` calls to catch completion.
   Polling too early wastes round-trips; polling too late leaves the assistant
   idle; doing other work and checking later risks acting on stale state or
   missing errors.
2. **No cross-tool coordination.** Each tool's `fetch` action queries a single
   handle.
   The assistant cannot express "wait for both handles to finish" or "wait for
   whichever finishes first," short of giving up on parallelism and running
   tools sequentially.

`await` replaces polling with an explicit synchronization point across handles.

### Examples

The assistant can spawn `cargo_check` and `cargo_test` and wait for both in a
single tool call batch:

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
IDs](#model-chosen-handle-ids)), it can pass them to `await` in the same batch
without an extra round-trip.

With `any`, the assistant can race two searches and take whichever finishes
first:

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

`await` is a built-in tool, like `describe_tools`.
Rather than belonging to an individual tool's action schema, it operates on the
handle registry across all stateful tools.

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

A call must provide `any`, `all`, or both.

#### Completion condition

`await` blocks until both conditions hold:

- Every handle in `all` (when provided) has reached `Stopped`.
- At least one handle in `any` (when provided) has reached `Stopped`.

Handles that are already `Stopped` when `await` is called count immediately.
If the completion condition is already satisfied at call time, `await` returns
without blocking.

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

`completed` lists all referenced handles in the `Stopped` state when `await`
returns, and `pending` lists handles still in `Running` or `Waiting`.
Handles from `any` and `all` are grouped together by state rather than by input
parameter.

Each stopped handle includes `result`, which holds the tool's output on success
or error message on failure, matching a standard `ToolCallResponse`.
Each pending handle (present in `any` mode when a subset of handles satisfies
the condition) reports its current state so the assistant can decide whether to
await again, `fetch` individually, or abort.

#### Timeout behavior

If `timeout_secs` is omitted, `await` waits without a time limit.
When `timeout_secs` expires before the completion condition is met, `await`
returns the current handle states across `completed` and `pending` with
`"timed_out": true`:

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

Timing out does not abort pending handles; they keep running and can be awaited
again or fetched individually.

#### Errors and edge cases

| Condition                         | Behavior                                 |
| --------------------------------- | ---------------------------------------- |
| Both `any` and `all` are empty or | Error: "At least one handle ID required" |
| missing                           |                                          |
| Handle ID not found in registry   | Error: "Handle `my_handle` not found"    |
| Handle is in `Waiting` state      | Counts as pending. JP continues handling |
|                                   | the inquiry/prompt while `await` waits.  |
|                                   | The handle transitions to `Stopped` once |
|                                   | the question is answered and the tool    |
|                                   | finishes.                                |
| All handles already stopped       | Immediate return, no blocking.           |

An unknown handle ID returns an error rather than being silently skipped,
surfacing typos and stale IDs immediately.

### Integration with the handle registry

`await` operates on the `HandleRegistry` from [RFD 009]:

1. Validate that all referenced handle IDs exist in the registry.
2. Return immediately if the completion condition is already met.
3. Subscribe to state-change notifications for the referenced handles.
4. Block asynchronously until the condition is met or `timeout_secs` expires.
5. Collect the current handle states from the registry and return.

To support step 3, each entry in `HandleRegistry` carries a
`tokio::sync::Notify` that fires when the handle reaches `Stopped`:

```rust
struct HandleEntry {
    handle: ToolHandle,
    /// Notified when the handle reaches a terminal state.
    notify: Arc<Notify>,
}
```

`Notify` supports multiple concurrent waiters, so multiple `await` calls can
watch the same handle.

### Extending `BuiltinTool` with an execution context

`BuiltinTool` is stateless:

```rust
pub trait BuiltinTool: Send + Sync {
    async fn execute(&self, arguments: &Value, answers: &IndexMap<String, Value>) -> Outcome;
}
```

`await` requires two pieces of per-execution state that this signature cannot
provide:

1. **Handle registry access**: to look up handles and subscribe to state-change
   notifications.
   The `HandleRegistry` ([RFD 009]) is shared state managed by the coordinator.
2. **Cancellation token**: to abort on turn end or user interrupt.
   `ToolExecutor.execute()` already holds a cancellation token and passes it to
   `execute_local` and `execute_mcp`, but drops it before `execute_builtin`.

This RFD adds a `BuiltinContext` parameter to `BuiltinTool::execute`:

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

`BuiltinContext` lives alongside `BuiltinTool` in `jp_llm::tool::builtin`.
Because the trait is internal to the workspace, the signature change is
contained:

- `DescribeTools::execute` accepts an unused `_ctx: &BuiltinContext` parameter.
- `ToolDefinition::execute_builtin` constructs `BuiltinContext` from state
  already in scope.
- Future built-in tools (such as sub-agents or tools inspecting conversation
  state) can extend `BuiltinContext` without changing the trait signature again.

Two alternatives that avoid changing the trait were considered and rejected:

- **Constructor injection.** Storing `Arc<HandleRegistry>` on `AwaitTool` at
  construction time works for the registry, but not for the per-call
  cancellation token.
  Passing the token would require the coordinator to mutate `AwaitTool` before
  each call.
- **Coordinator interception.** Special-casing `tool_name == "await"` in the
  coordinator bypasses `BuiltinTool` entirely, but every future stateful
  built-in tool would require its own special case.

### Model-chosen handle IDs

In [RFD 009], JP generates handle IDs (`h_1`, `h_2`) and returns them to the
assistant, forcing an extra round-trip before the assistant can reference a
spawned handle.
Reusing the LLM provider's tool call ID is not feasible either, because most
providers assign tool call IDs at the API layer rather than letting the model
choose them.

This RFD instead requires the assistant to pass a descriptive `id` (`check`,
`test`, `build_release`) on the `spawn` action, replacing [RFD 009]'s generated
IDs:

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

Choosing the ID up front lets the assistant spawn and await handles in a single
batch, and keeps handle references readable in conversation history:

```txt
A: [
  call(cargo_check, { action: "spawn", id: "check" }),
  call(cargo_test,  { action: "spawn", id: "test" }),
  call(await, { all: ["check", "test"] })
]
```

All three calls execute concurrently: the spawns register their handles and
`await` blocks until both reach `Stopped`.
JP validates that each `id` is unique among active handles and returns an error
on collision.

### `await` in parallel tool call batches

`ToolCoordinator` executes tool calls in a single LLM response concurrently.
To make same-batch `spawn` and `await` calls race-free, the coordinator
pre-registers handle IDs before dispatching the batch:

1. Scan the batch for `spawn` actions.
2. Pre-register each spawn's `id` in the handle registry as a placeholder entry
   in the `Pending` state (with no running process yet).
3. Dispatch all tool calls in the batch concurrently.

When `await` looks up a handle ID, the placeholder entry already exists even if
the corresponding `spawn` task has not started running.
`await` subscribes to the entry's `Notify` and waits for the handle to
transition through `Running` to `Stopped`.

Pre-registration also catches ID collisions before any tool in the batch runs:
if two spawns in the batch share an `id`, or if a spawn reuses an active `id`
from an earlier batch, the coordinator rejects the entire batch immediately.

### Schema availability

JP exposes `await` in the tool list only when at least one stateful tool is
configured, mirroring how `describe_tools` is included only when tool
documentation exists.
When no stateful tools are active, `await` is omitted.

### Spawn configuration

Per-tool stateful handle behavior is configured under
`[conversation.tools.<name>.spawn]`:

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

Notifications are delivered via the system message queue ([RFD 011]).
Each flag controls whether a handle state change queues a notification:

| Flag         | Triggers when                         | Default |
| ------------ | ------------------------------------- | ------- |
| `on_success` | Handle reaches `Stopped` with `Ok`    | `true`  |
|              | result                                |         |
| `on_failure` | Handle reaches `Stopped` with `Err`   | `true`  |
|              | result                                |         |
| `on_waiting` | Handle enters `Waiting` (needs input) | `true`  |
| `on_content` | Handle produces new output while      | `false` |
|              | `Running`                             |         |

`on_content` defaults to `false` because build tools would otherwise queue a
notification for every line of output; enable it for tools where incremental
output matters, such as test runners or log tailers.

Notifications are sent only for handles the assistant has not explicitly polled
with `fetch` or awaited.
While the assistant is actively watching a handle, its notifications are
suppressed.

#### Turn-end behavior

[RFD 009] aborts all running handles when a turn ends.
This RFD makes turn-end handling configurable via `on_turn_end`:

**`inquire`** (default): JP sends an `InquiryRequest` to the assistant with a
dynamically generated schema listing each active handle, its state, and a
trimmed output preview.
The assistant decides per handle whether to wait or abort:

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

The inquiry target can be overridden via `AssistantOverrideConfig` ([RFD 034])
to route this classification step to a smaller, cheaper model.
For handles set to `wait`, JP blocks until they reach `Stopped` (up to a
configurable timeout), delivers the results as `ToolCallResponse` events, and
resumes the turn so the assistant can respond.
For handles set to `abort`, JP terminates them immediately.

**`await`**: JP waits for all active handles up to a configurable timeout
without an extra LLM round-trip.
Completed results are queued on the system message queue for the next turn;
handles still running when the timeout expires are aborted.

**`abort`**: JP terminates all active handles immediately and discards their
output, matching [RFD 009].
Use this for tools where results are no longer useful once the turn ends.

## Drawbacks

- **Token and round-trip overhead.** Each `await` call is an additional tool
  call.
  When the assistant spawns two tools and awaits them without doing intervening
  work, `await` adds overhead compared to running one-shot tools in parallel.
  The stateful pattern pays off only when the assistant interleaves work between
  spawning and awaiting, or races alternatives with `any`.
- **Coordinator complexity.** `ToolCoordinator` is already the most complex
  module in the query pipeline, and supporting `await` alongside stateful and
  one-shot tool dispatch adds another execution path.
- **Model planning burden.** Multi-step asynchronous workflows are harder for
  models to plan than one-shot tool calls.
  Even with system prompt guidance, weaker models may misuse or underuse the
  `spawn` and `await` pattern.

## Alternatives

### Rely on the system message queue ([RFD 011])

Let the assistant spawn tools and continue working while JP delivers "tool
stopped" notifications on subsequent messages via the system message queue.

Rejected as a sole mechanism because the system message queue is passive: the
assistant cannot block until a result is ready or express `any` and `all`
conditions across handles.
The two mechanisms are complementary: the message queue covers background
handles whose completion timing is not critical, while `await` provides explicit
synchronization.

### Extend `fetch` to accept multiple handle IDs

Extend each stateful tool's `fetch` action to take an array of handle IDs and
block until they complete.

Rejected because `fetch` belongs to an individual tool's action schema ([RFD
009]), whereas synchronization often spans handles from different tools (for
example, `cargo_check` and `cargo_test`).

### Implicit parallelism

Run independent tool calls in parallel automatically and return all results at
once, without explicit `spawn` and `await` calls.

`ToolCoordinator` already executes one-shot tool calls in a batch concurrently.
The stateful protocol targets workflows that one-shot batching cannot express:
long-running processes, interactive sessions, racing alternative tools, and
interleaving work between spawning a tool and collecting its result.

### Separate `await_all` and `await_any` tools

Expose two tools (`await_all` and `await_any` or `select`) instead of a single
`await` tool.

Rejected because a single `await` tool with `any` and `all` parameters keeps the
tool surface smaller and supports combining both conditions in one call.

## Non-Goals

- **Inter-handle pipelines.** Piping output from one handle into another (such
  as feeding `cargo check` output into a formatter) is out of scope.
- **Automatic abort on `any` completion.** When an `any` condition is met,
  remaining handles keep running unless the assistant aborts them explicitly.
- **Handle priority or ordering.** All handles are treated equally; there is no
  tie-breaking preference when multiple handles complete simultaneously.
- **Spawning `await` itself.** `await` is not a stateful tool and cannot be
  spawned or awaited recursively.
  It executes directly within the tool call batch.

## Risks and Open Questions

### Handle ID collisions

Because the assistant chooses handle IDs, a `spawn` call can collide with
another `spawn` in the same batch or with a handle still active from an earlier
batch.
Batch pre-registration detects both cases before any tool executes and rejects
the batch with an error naming the conflicting `id` so the assistant can retry.

### Provider support for `anyOf` in tool schemas

The `await` parameter schema uses top-level `anyOf` with `required` to enforce
that at least one of `any` or `all` is provided.
If an LLM provider rejects top-level `anyOf`, JP can mark both properties as
optional in the schema and validate that at least one is present at runtime.

### `Waiting` handles and the inquiry system

A handle in the `Waiting` state is blocked on a question routed either to the
user via an interactive terminal prompt or to the inquiry backend via a
structured LLM call ([RFD 028]).
`await` treats `Waiting` as a non-terminal state alongside `Running` and
continues blocking while JP resolves the question:

- **User-targeted questions:** JP prompts the user in the terminal, delivers the
  answer to the tool, and waits for the handle to reach `Stopped`.
- **Assistant-targeted questions:** The inquiry backend runs a structured LLM
  call ([RFD 028]) and feeds the answer back to the tool.

If the inquiry fails or the user cancels the prompt, the handle still
transitions to `Stopped` with an error result.
If an interactive prompt blocks indefinitely because the user is away from the
terminal, `timeout_secs` bounds how long `await` waits.

### Tool permission prompts (`RunMode::Ask`)

When the assistant spawns a tool configured with `RunMode::Ask` and awaits it in
the same batch, the permission prompt blocks the `spawn` task before the process
starts.
`await` waits on the pre-registered `Pending` handle until the user approves the
prompt and the tool runs to `Stopped`, though a slow approval can make `await`
appear stuck unless `timeout_secs` is set.

## Implementation Plan

### Phase 0: `BuiltinContext` and trait change

Add `BuiltinContext` (initially containing `cancellation_token:
CancellationToken`) to `jp_llm::tool::builtin` and pass `&BuiltinContext` to
`BuiltinTool::execute`.
Update `DescribeTools` to accept the new parameter, and thread the cancellation
token from `ToolDefinition::execute` through `execute_builtin`.

This phase has no dependency on [RFD 009] and can be merged independently to
make built-in tools cancellable.

### Phase 1: Handle notification infrastructure

Extend `HandleRegistry` ([RFD 009] Phase 2) with per-handle `Notify` instances
that wake waiters when a handle reaches `Stopped`, and add `handle_registry:
Option<Arc<HandleRegistry>>` to `BuiltinContext`.

Depends on Phase 0 and [RFD 009] Phase 2.
Can be merged independently with no user-visible behavior change.

### Phase 2: `await` execution and coordinator dispatch

Implement the `await` tool and wire it into `ToolCoordinator`: validate handle
IDs, check the completion condition, subscribe to handle notifications, wait for
completion or timeout, and return the structured JSON `ToolCallResponse`.

Depends on Phase 1 and [RFD 009] Phase 4 (stateful tool dispatch).

### Phase 3: Schema and conditional exposure

Expose `await` and its parameter schema in the tool list whenever stateful tools
are configured, and add system prompt instructions for the `spawn` and `await`
workflow.

Depends on Phase 2.

### Phase 4: Same-batch `spawn` and `await`

Pre-register handle IDs in `ToolCoordinator` by scanning each tool call batch
for `spawn` actions and inserting `Pending` placeholder entries in
`HandleRegistry` before dispatching the batch.

Depends on Phase 2 and can be merged independently of Phase 3.

## References

- [RFD 009: Stateful Tool Protocol][RFD 009]: the handle registry and stateful
  tool model that `await` builds on.
- [RFD 011: System Notification Queue][RFD 011]: complementary notification
  mechanism for handles that finish without being awaited.
- [Query Stream Pipeline]: the turn loop and tool coordinator where `await` is
  dispatched.

[Query Stream Pipeline]: ../wiki/00dxyb6-query-stream-pipeline-architecture.md
[RFD 009]: 009-stateful-tool-protocol.md
[RFD 011]: 011-system-notification-queue.md
[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 034]: 034-inquiry-specific-assistant-configuration.md
