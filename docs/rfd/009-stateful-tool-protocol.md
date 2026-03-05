# RFD 009: Stateful Tool Protocol

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-23

## Summary

This RFD introduces a stateful tool execution protocol that unifies one-shot and
long-running tools under a single execution model. Every tool call is internally
modeled as a state machine (`Running → Stopped`), with one-shot tools
wrapped transparently. Long-running tools expose this lifecycle to the
assistant, enabling multi-step interactive workflows like `git add --patch` or
persistent shell sessions.

## Motivation

JP's current tool execution model is synchronous: the assistant calls a tool, JP
runs a command, captures the output, and returns it. This works for tools like
`cargo_check` or `grep`, but breaks down for programs that:

- Run indefinitely (shells, file watchers, dev servers)
- Require multiple rounds of input (interactive git, debuggers)
- Produce output over time (build processes, test suites)
- Need to be inspected or stopped mid-execution

Today, if a tool takes too long or needs input, it hangs. There is no way for
the assistant to check on a running tool, send it input, or stop it. The only
escape hatch is the [`inquiry` system], which re-executes the tool from scratch
with accumulated answers — a pattern that doesn't work for tools with genuine
long-running state.

We want a model where:

1. Any tool can run asynchronously without blocking the conversation.
2. The assistant can spawn a tool, check its state, send input, and stop it.
3. One-shot tools work exactly as they do today — no user-visible change.
4. The same protocol supports both simple commands and interactive programs.

[`inquiry` system]: ./005-first-class-inquiry-events.md

### Concrete example

Consider a `git` tool that supports interactive staging:

```txt
assistant: call(git, { action: "spawn", command: "stage", args: ["--patch"] })
  → { "id": "h_1", "state": "running", "content": "diff --git a/..." }

assistant: call(git, { action: "fetch", id: "h_1" })
  → { "id": "h_1", "state": "running", "content": "Stage this hunk? [y/n/...]" }

assistant: call(git, { action: "apply", id: "h_1", input: "y" })
  → { "id": "h_1", "state": "running", "content": "Next hunk: ..." }

assistant: call(git, { action: "fetch", id: "h_1" })
  → { "id": "h_1", "state": "stopped", "result": "Staged 3 hunks." }
```

Today this is impossible. The `git` tool can only run non-interactive commands.
With the stateful protocol, the tool author builds a `git` tool that uses JP's
infrastructure to manage the interactive subprocess, and the assistant drives
the workflow through the standard tool call interface.

### Toward sub-agents

The stateful tool protocol is also a stepping stone toward sub-agent
capabilities in JP, where the main assistant spawns sub-assistants to perform
tasks concurrently. A sub-agent is, from the protocol’s perspective, just
another stateful tool: it is spawned, produces output over time, accepts input,
and eventually stops. The handle registry, action-based schema, and
assistant-driven polling model all apply directly. This RFD does not propose
sub-agents, but the infrastructure it introduces is designed to support them.

## Design

### `ToolState` — what a tool produces

`ToolState` replaces the current `Outcome` enum. It represents the state of a
tool at any point in its lifecycle:

```rust
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolState {
    /// The tool is running. Contains optional output produced so far,
    /// including any initial output from startup.
    Running { content: Option<String> },

    /// The tool is paused, waiting for structured input.
    ///
    /// This maps to the existing inquiry system. `Apply` delivers
    /// the answer to the pending question.
    Waiting { content: Option<String>, question: Question },

    /// The tool has stopped (successfully or with an error).
    Stopped { result: Result<String, ToolError> },
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolError {
    pub message: String,
    pub trace: Vec<String>,
    pub transient: bool,
}
```

All states carry an optional `content: String` field. The tool author controls
what goes in this string — they might format it as plain text, JSON, XML, or
combine stdout/stderr however they see fit.

`Running` includes content from the first response onward, including any
initial output from startup (a shell prompt, an editor screen, a welcome
message). This avoids an extra round-trip to get the initial content.

`Stopped` returns either a successful output as a string, or an error message
containing an optional trace and a boolean indicating whether the error is
transient (e.g. a network error), and can thus be retried.

#### Why `content` is a string, not a generic or `Value`

Structured data that JP needs to act on (a `Question` in `Waiting`, a
`ToolError` in `Stopped`) is carried in dedicated typed fields on each variant,
not inside `content`. The `content` field is opaque output intended for the
assistant — JP passes it through without parsing it.

This means future variants can carry additional typed fields without changing
`content`'s type. For example, a sub-agent tool that surfaces an inquiry would
return `Waiting { question, .. }` with the `Question` in the typed field. The
sub-agent serializes its internal state as JSON over the wire; JP deserializes
it into the typed `ToolState` variants on the other side — the same pattern
that local tools already use when they write `Outcome` JSON to stdout.

Making `content` into `serde_json::Value` or making `ToolState` generic over
`T: Serialize` was considered but rejected: `Value` forces every consumer to
parse, and generics infect every containing type with a type parameter. In
practice, content is almost always a string. If a richer wire format is needed
for sub-agents, that can be addressed in the sub-agent RFD by evolving the
variants, not by changing `content`'s type.

### State transitions

```txt
Running → Stopped
   ↓
 Waiting → Running (after Apply)
   ↓
 Stopped (if aborted while waiting)
```

`Running` is the initial state for a stateful tool. It means the tool is active
and may contain output. `Waiting` means the
tool needs structured input (a `Question`) before it can continue — this maps to
the existing inquiry system. `Stopped` is terminal — the tool has exited, either
successfully (`Ok(content)`) or with an error (`Err(...)`).

### `ToolCommand` — what JP dispatches internally

When the assistant calls a tool, JP parses the arguments and produces a
`ToolCommand`:

```rust
pub enum ToolCommand {
    /// Spawn a new tool process.
    Spawn { args: Map<String, Value> },

    /// Read the current state of a running tool.
    Fetch { id: String },

    /// Send input to a running or waiting tool.
    Apply { id: String, input: Value },

    /// Terminate a running tool.
    Abort { id: String },
}
```

For one-shot tools (no `action` field in arguments), JP synthesizes a `Spawn`
followed by a blocking `Fetch` loop until the tool reaches `Stopped`. The
assistant never sees intermediate states.

For stateful tools (the assistant includes an `action` field), JP dispatches the
command directly and returns the resulting `ToolState` to the assistant.

#### `Apply` serves two purposes

`Apply { input }` is used for both:

1. **Sending data to a `Running` tool.** The `input` value (typically a string)
   is written to the tool's stdin or input mechanism. For example, sending
   `"y\n"` to a `git add --patch` session.

2. **Answering a `Question` from a `Waiting` tool.** The `input` value (a
   boolean, string, or other typed value) is delivered as the answer to the
   pending question. This is the same data that the inquiry system or user
   prompt would produce.

The tool handle's internal state determines how `Apply` is routed:

- **`Running` handle**: `input` is written to stdin. No question correlation
  needed.
- **`Waiting` handle**: JP extracts the `question.id` from the handle's
  `Waiting { question, .. }` state and maps the answer:
  `accumulated_answers[question.id] = input`. This is unambiguous because a
  handle has at most one pending question — a tool can only be in one state
  at a time.

The caller (assistant or JP's internal one-shot wrapper) does not need to
include a question ID in the `Apply` command. JP derives it from the handle's
current state, preserving the same correlation the existing inquiry system
provides through explicit `InquiryId` matching.

For one-shot shell tools in the `Waiting` state, `Apply` triggers a re-execution
with accumulated answers (preserving the existing `NeedsInput` behavior). For
stateful tools, `Apply` delivers the input to the still-running process.

#### Per-tool action sets

Not every stateful tool supports every action. A tool declares which actions it
supports, and only those appear in the schema exposed to the assistant:

| Tool                  | Actions                            | Why                                 |
|-----------------------|------------------------------------|-------------------------------------|
| `git` (interactive)   | `spawn`, `fetch`, `apply`, `abort` | Needs input for interactive staging |
| `cargo_check` (async) | `spawn`, `fetch`, `abort`          | No stdin, just poll for completion  |
| Background task       | `spawn`, `fetch`                   | Fire and forget, can’t be stopped   |

The assistant *cannot* call an action that the tool doesn’t expose — it’s not in
the schema. This avoids the question of what happens when `apply` is sent to a
tool that doesn’t accept input: that case simply cannot arise.

#### Mapping from current `Outcome`

| Current `Outcome`                     | New `ToolState`                     |
|---------------------------------------|-------------------------------------|
| `Success { content }`                 | `Stopped { result: Ok(_) }`         |
| `Error { message, trace, transient }` | `Stopped { Result: Err(_) }`        |
| `NeedsInput { question }`             | `Waiting { content: "", question }` |

### How the assistant interacts with stateful tools

A stateful tool exposes `action` and `id` in its JSON schema, alongside its
own tool-specific parameters. The schema uses `oneOf` so each action variant
has its own sub-schema. Only the actions the tool supports are included.

Example schema for `git` (supports `spawn`, `fetch`, `apply`, `abort`):

```json
{
  "type": "object",
  "oneOf": [
    {
      "properties": {
        "action": {
          "const": "spawn"
        },
        "command": {
          "enum": [
            "stage",
            "commit",
            "rebase"
          ]
        },
        "args": {
          "type": "array",
          "items": {
            "type": "string"
          }
        }
      },
      "required": [
        "action",
        "command"
      ]
    },
    {
      "properties": {
        "action": {
          "const": "fetch"
        },
        "id": {
          "type": "string"
        }
      },
      "required": [
        "action",
        "id"
      ]
    },
    {
      "properties": {
        "action": {
          "const": "apply"
        },
        "id": {
          "type": "string"
        },
        "input": {}
      },
      "required": [
        "action",
        "id",
        "input"
      ]
    },
    {
      "properties": {
        "action": {
          "const": "abort"
        },
        "id": {
          "type": "string"
        }
      },
      "required": [
        "action",
        "id"
      ]
    }
  ]
}
```

Example schema for `cargo_check` (supports `spawn`, `fetch`, `abort` — no
`apply` because it doesn’t accept input):

```json
{
  "type": "object",
  "oneOf": [
    {
      "properties": {
        "action": {
          "const": "spawn"
        },
        "package": {
          "type": "string"
        }
      },
      "required": [
        "action"
      ]
    },
    {
      "properties": {
        "action": {
          "const": "fetch"
        },
        "id": {
          "type": "string"
        }
      },
      "required": [
        "action",
        "id"
      ]
    },
    {
      "properties": {
        "action": {
          "const": "abort"
        },
        "id": {
          "type": "string"
        }
      },
      "required": [
        "action",
        "id"
      ]
    }
  ]
}
```

The tool author defines the `spawn` variant’s parameters and which actions are
supported. The `fetch`, `apply`, and `abort` variants follow a fixed pattern.
JP’s SDK generates the full schema from the tool’s definition for tools written
in Rust. Other languages may need to generate the schema manually.

### Handle registry

JP maintains a handle registry that maps handle IDs to running tool instances.
The registry lives in the query execution loop and persists across tool call
rounds within a turn.

```rust
struct HandleRegistry {
    handles: HashMap<String, ToolHandle>,
    next_id: u64,
}

struct ToolHandle {
    tool_name: String,
    state: ToolState,
    // Opaque handle to the running tool — the tool implementation
    // decides what this contains (a child process, a PTY session,
    // an async task, etc.)
    inner: Box<dyn AnyToolHandle>,
}
```

Handle IDs are JP-generated (`h_1`, `h_2`, ...), monotonically increasing
within a JP process. The tool itself never generates or sees its own handle
ID.

When the assistant calls `tool(action: "fetch", id: "h_1")`, JP looks up
`h_1` in the registry, calls the tool's fetch handler, and returns the result.

### Handle lifecycle

Handles are created when a tool returns a non-terminal state and destroyed
when:

- The tool reaches `Stopped` (natural completion)
- The assistant or user sends `Abort`
- The conversation turn ends (all remaining handles are aborted)
- JP exits (child processes receive SIGTERM, then SIGKILL)

### One-shot tool wrapping

For tools that don't declare stateful support, JP wraps the existing execution
in the stateful protocol:

```txt
assistant: call(cargo_check, { package: "jp_cli" })

JP internally:
  1. No `action` in args → treat as one-shot
  2. Spawn tool → internally creates handle
  3. Loop: fetch handle state
     - Running → wait, continue
     - Waiting → handle via inquiry/prompt (existing NeedsInput flow)
     - Stopped → return result to assistant
  4. Destroy handle
  5. Return ToolCallResponse to assistant as before

assistant gets: ToolCallResponse { result: "ok, 0 warnings" }
```

The assistant sees no difference. The `StreamEventHandler.handle_tool_call`
function wraps the existing call in this loop. The `NeedsInput` -> re-execute
pattern maps to: tool returns `Waiting`, JP collects the answer (prompt or
inquiry), sends `Apply`, tool continues.

For shell-based tools that exit immediately, the internal flow is: spawn
process → process exits → parse stdout as `Outcome` → convert to
`ToolState::Stopped` → return. The handle exists for microseconds.

### Detecting stateful vs. one-shot tools

A tool is stateful if the assistant invokes it using the stateful protocol —
that is, with an `action` field in its arguments. Detection is based on the
**invocation**, not the return value.

- **`action` field present** → stateful. JP dispatches the `ToolCommand`
  directly. If the action is `spawn`, JP registers a handle for the tool.
  Subsequent `fetch`/`apply`/`abort` calls reference the handle by ID.
- **No `action` field** → one-shot. JP wraps the call in the internal
  spawn/fetch loop. If the tool returns `Waiting` (e.g., `fs_modify_file`
  asking about a dirty file), JP handles it via the existing inquiry/prompt
  path and re-executes with accumulated answers. The tool is never registered
  as stateful.

This distinction matters because a one-shot shell tool that returns `Waiting`
has already exited — there is no running process to send `Apply` to. The
`Waiting` state in the one-shot path means "re-execute with answers" (the
current `NeedsInput` behavior), while `Waiting` in the stateful path means
"the process is alive and paused, deliver input via `Apply`."

Existing tools like `fs_modify_file` require **no changes**. They don't
declare stateful actions in their schema, so the assistant never sends an
`action` field, and JP runs them through the one-shot path exactly as today.

For the JSON schema to include the `action`/`id` fields, the tool's
configuration or definition must indicate stateful support. This could be:

- A flag in the tool config (`stateful = true`)
- The tool definition including `action` in its parameters
- Built-in tools declaring it programmatically

The exact mechanism for schema declaration is a detail to resolve during
implementation.

### Integration with existing tool system

The stateful protocol slots into the existing execution flow at the
`StreamEventHandler.handle_tool_call` level. Today this method:

1. Resolves the tool config and definition
2. Handles run mode (ask/unattended/edit/skip)
3. Calls `ToolDefinition::call` in a loop (retrying on `NeedsInput`)
4. Collects `ToolCallResponse`

With the stateful protocol, step 3 changes:

- **One-shot tools**: Same loop, but internally modeled as
  spawn → fetch → stopped. The `NeedsInput` retry loop maps to the
  `Waiting` → `Apply` cycle.
- **Stateful tools**: No loop. JP dispatches the `ToolCommand`, returns the
  `ToolState` to the assistant, and the assistant drives subsequent calls.

The `ToolCallResponse` sent to the assistant for stateful tools includes the handle
ID and state:

```rust
ToolCallResponse {
    id: call.id,
    result: Ok(json!({
        "id": "h_1",
        "state": "running",
        "content": "Stage this hunk? [y/n/...]"
    }).to_string()),
}
```

The conversation event stream (`ToolCallRequest` → `ToolCallResponse`) is
unchanged. Each `action` call is a separate tool call round from the assistant's
perspective.

### Interaction with the inquiry system

The current inquiry system handles `NeedsInput` with
`QuestionTarget::Assistant` by making a structured LLM call to get the answer,
then re-executing the tool. Under the stateful protocol, the inquiry system
produces the answer the same way, but delivers it via `Apply`:

1. Tool returns `Waiting { question, .. }` (with `target: Assistant` in config)
2. JP's inquiry system makes the structured LLM call, extracts the answer
   (unchanged)
3. JP sends `Apply { input: answer }` to the tool handle
4. For one-shot shell tools: `Apply` triggers re-execution with accumulated
   answers (existing behavior — the process has already exited)
5. For stateful tools: `Apply` delivers the answer to the still-running process

When the assistant drives a stateful tool directly (no inquiry system):

1. Tool returns `Waiting { question, .. }` → JP returns it to the assistant
   as part of the `ToolCallResponse`
2. The assistant calls `tool(action: "apply", id: "h_1", input: true)`
3. JP sends `Apply { input: true }` to the handle
4. The tool answers the question and continues

Both paths use the same `Apply` command. The tool handle routes the input
based on its internal state (stdin write vs. question answer).

### `ToolState` in `jp_tool`

The `Outcome` type in `jp_tool` is the public API that tool authors use. The
migration path:

1. Add `ToolState` alongside `Outcome` in `jp_tool`
2. Implement `From<Outcome> for ToolState` for backward compatibility
3. Update internal code to use `ToolState`
4. Deprecate `Outcome` (but keep accepting its JSON format from shell tools)

```rust
impl From<Outcome> for ToolState {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Success { content } => ToolState::Stopped {
                content,
                error: None,
            },
            Outcome::Error { message, trace, transient } => ToolState::Stopped {
                content: String::new(),
                error: Some(ToolError { message, trace, transient }),
            },
            Outcome::NeedsInput { question } => ToolState::Waiting {
                content: String::new(),
                question,
            },
        }
    }
}
```

## Drawbacks

**Complexity.** The unified state machine adds a layer of abstraction over
what is currently a simple call-and-return model. For the majority of tools
that are one-shot, this abstraction provides no user-visible benefit — it's
purely internal.

**Handle management.** Long-lived handles introduce state that must be tracked,
cleaned up on errors, and survived across tool call rounds. This is new
territory for JP's tool system, which currently has no cross-round state beyond
`TurnState.pending_tool_call_questions`.

**Schema complexity.** The `oneOf` schema for stateful tools is more complex
than a flat parameter list. Some assistant providers handle `oneOf` schemas poorly
or not at all. This may require provider-specific schema transformations.

**Two execution paths.** Despite the unified model, one-shot and stateful tools
still have different code paths (wrap-and-loop vs. direct dispatch). The
abstraction unifies the types, not the implementation.

## Alternatives

### Keep tools strictly one-shot, add separate "session" tools

This was the original approach in the first draft of this RFD — expose PTY
sessions as a set of generic `terminal_*` tools. The assistant would call
`terminal_start("git add --patch")` to launch any interactive program.

**Rejected because:** It exposes the wrong abstraction. The assistant should call
domain tools (`git`, `editor`), not generic terminal tools. The stateful
protocol lets each tool define its own commands and parameters while reusing
JP's handle management infrastructure.

### Make statefulness a tool config property only

Require `stateful = true` in the tool configuration, rather than inferring it
from the tool's return value.

**Rejected because:** It adds configuration burden. However, some form of
declaration is needed for schema generation — the assistant needs to see the
`action`/`id` parameters in the tool's schema to know it supports the stateful
protocol. The rejected approach is making config the *only* mechanism. In the
proposed design, the schema declaration drives both schema generation and
runtime dispatch (presence of `action` in arguments).

### Separate `ToolState` from `Outcome` entirely

Don't provide `From<Outcome> for ToolState`. Make them independent types with
no conversion path.

**Rejected because:** It would break every existing tool immediately. The
conversion preserves backward compatibility — existing tools that output
`Outcome` JSON continue to work.

## Non-Goals

- **PTY / terminal emulation.** This RFD covers the protocol and handle
  management. How a tool actually manages an interactive subprocess (PTY,
  terminal emulator, screen buffer) is covered in a follow-up RFD.
- **Interactive tool SDK.** A convenience SDK (`jp_tool::interactive`) for
  building stateful tools in Rust is a follow-up. This RFD defines the
  protocol that such an SDK would target.
- **Parallel stateful tools.** Multiple handles can coexist, but this RFD does
  not address coordinating between them (e.g., piping output from one handle
  to another).
- **Persistent handles across JP invocations.** Handles are scoped to the JP
  process. When JP exits, all handles are destroyed.

## Risks and Open Questions

### Schema generation for stateful tools

The `oneOf` schema pattern for `action` variants is not universally supported by
LLM providers. Google's Gemini supports `anyOf` but [strictly requires it to be
the only field in the schema object][gemini-anyof] — no sibling properties are
allowed alongside it. Other providers may have their own limitations. We may
need to fall back to a flat schema with optional fields and use `description` to
explain the action protocol, or apply per-provider schema transformations. This
needs validation with each provider.

[gemini-anyof]: https://github.com/anomalyco/opencode/issues/14509

### Handle cleanup guarantees

If JP crashes (SIGKILL), handles are not cleaned up. Child processes become
orphans. This is the same risk as the current subprocess model, but stateful
tools are more likely to have long-running processes that leave visible state
(lock files, half-written files). We should document this and consider a
cleanup-on-startup mechanism (e.g., a `.jp/handles.lock` file).

### Token cost of stateful interactions

Each `fetch` / `apply` round is a full LLM tool call round-trip. A 20-step
interactive session means 20 tool call rounds, each adding input/output tokens.
For tools that produce large output (full terminal screens), this adds up. This
RFD does not address token optimization — that's a concern for the tool
implementation (e.g., returning diffs instead of full screens).

### Interaction with tool permission system

The current `RunMode::Ask` prompts the user before each tool call. For stateful
tools, this would mean prompting before every `fetch` and `apply`, which would
make interactive workflows unusable. The recommendation is: prompt on `spawn`,
run `fetch`/`apply`/`abort` unattended. But this should be configurable per
action, not just per tool.

### Proactive delivery of stopped handles

When a stateful tool reaches `Stopped` without the assistant having asked for
its state (e.g., `cargo check` finishes while the assistant is doing other
work), how does the assistant learn about it?

The current event model requires `ToolCallRequest` → `ToolCallResponse` pairs.
JP cannot inject a response without a corresponding request. Options
considered:

- **Require the assistant to poll.** Simple but fragile — the assistant must
  remember to `fetch` every handle it spawned.
- **Fabricate a `ToolCallRequest`/`ToolCallResponse` pair.** Violates the event
  model contract.
- **Deliver at turn end.** The turn is already over — the assistant has
  responded. The result would only be available in the next turn.

None of these are clean. The recommended approach for this RFD is
**assistant-driven polling**: the assistant is responsible for checking on its
handles. JP does not proactively push results.

A better solution may be a general-purpose **system message queue** (explored
in a separate RFD). The idea: JP maintains a queue of system notifications
that are delivered to the assistant at the next available communication
opportunity:

- In any `ToolCallResponse` about to be sent
- When the user interrupts the stream and triggers an in-turn `ChatRequest`
- When the turn ends

At those points, JP checks for queued notifications and prepends them to the
message. Notifications could include handle state changes, but also other
system events (MCP server disconnections, workspace changes, etc.).

This mechanism would be configurable per-tool: which notification types a
tool can emit (`state_stopped`, `state_waiting`), and at which delivery
points. This keeps the stateful tool protocol simple (poll-based) while
offering a path to proactive delivery without bending the event model.

### Backward compatibility of `ToolState` serialization

The `ToolState` JSON format (with `"type": "spawned"` etc.) is different from
the current `Outcome` format (with `"type": "success"` etc.). JP needs to accept
both formats from tool stdout. The detection heuristic: if the `type` field is
one of `spawned`, `running`, `waiting`, `stopped`, parse as `ToolState`; if it's
`success`, `error`, `needs_input`, parse as `Outcome` and convert.

## Implementation Plan

### Phase 1: `ToolState` type in `jp_tool`

Add the `ToolState` and `ToolError` types alongside existing `Outcome`. Add
`From<Outcome> for ToolState`. Add the backward-compatible deserialization
that accepts both formats. Unit tests for conversion and serialization.

Can be merged independently. No behavioral changes.

### Phase 2: `ToolCommand` type and handle registry

Add `ToolCommand` to `jp_tool`. Create a `HandleRegistry` struct (likely in a
new `jp_tool::handle` module or in `jp_cli`). Define the `AnyToolHandle` trait
that tool implementations will implement.

Can be merged independently. No behavioral changes yet — the types exist but
aren't used.

### Phase 3: One-shot tool wrapping

Refactor `StreamEventHandler.handle_tool_call` to use the stateful protocol
internally. Replace the `NeedsInput` retry loop with the spawn → fetch →
apply cycle. The `ToolDefinition::call` function returns `ToolState` instead of
`ToolCallResponse`. The wrapping loop converts `ToolState::Stopped` to
`ToolCallResponse`.

This is the critical integration phase. Existing behavior must be preserved
exactly. Extensive testing against the existing tool call test cases.

Depends on Phase 1 and 2.

### Phase 4: Stateful tool dispatch

Add the `action` / `id` argument parsing. When a tool call includes `action`,
JP dispatches the `ToolCommand` to the handle registry instead of wrapping in
a one-shot loop. Return `ToolState` as the tool call response content.

Depends on Phase 3.

### Phase 5: Schema generation for stateful tools

Add utilities to generate the `oneOf` schema from a tool's declared action set.
Integrate with `ToolDefinition::to_parameters_schema`. Handle provider-specific
schema limitations.

Depends on Phase 4. Can be iterated on independently.

## References

- [RFD 028: Structured Inquiry System](028-structured-inquiry-system-for-tool-questions.md)
  — the inquiry system that handles `Waiting` with `QuestionTarget::Assistant`.
- [Query Stream Pipeline](../architecture/query-stream-pipeline.md) — the turn
  loop and tool execution flow that this RFD modifies.
- [#392](https://github.com/dcdpr/jp/issues/392) — PTY-based end-to-end CLI
  testing (related infrastructure).
- [interminai](https://github.com/mstsirkin/interminai) — prior art for
  PTY-based tool interaction, validates the approach.
