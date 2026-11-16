# RFD 009: Stateful Tool Protocol

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-23
- **Required by**: [RFD 010], [RFD 037]
- **Summary**: Unifies one-shot and long-running tool execution under a stateful
  `Running`/`Stopped` protocol for multi-step interactive tools.

## Summary

This RFD introduces a stateful protocol that unifies one-shot and long-running
tools under a single execution model.
Every tool call is a state machine (`Running → Stopped`): one-shot tools are
wrapped transparently, while long-running tools expose spawn, input, and stop
actions for interactive workflows like `git add --patch`.

## Motivation

JP's current tool execution model is synchronous: the assistant calls a tool, JP
runs a command, captures the output, and returns it.
This works for tools like `cargo_check` or `grep`, but breaks down for programs
that:

- Run indefinitely (shells, file watchers, dev servers)
- Require multiple rounds of input (interactive git, debuggers)
- Produce output over time (build processes, test suites)
- Need to be inspected or stopped mid-execution

Today, if a tool takes too long or needs input, it hangs.
The assistant cannot check on a running tool, send it input, or stop it.
The only escape hatch is the [`inquiry` system], which re-executes the tool from
scratch with accumulated answers.
That pattern fails for tools with long-running state.

We want a model where:

1. Any tool can run asynchronously without blocking the conversation.
2. The assistant can spawn a tool, check its state, send input, and stop it.
3. One-shot tools work as they do today, with no user-visible change.
4. The same protocol supports both simple commands and interactive programs.

### Concrete example

Consider a `git` tool that supports interactive staging:

```txt
assistant: call(git, { action: "spawn", id: "staging", command: "stage", args: ["--patch"] })
  → { "id": "staging", "state": "running", "content": "diff --git a/..." }

assistant: call(git, { action: "fetch", id: "staging" })
  → { "id": "staging", "state": "running", "content": "Stage hunk? [y/n/...]" }

assistant: call(git, { action: "apply", id: "staging", input: "y" })
  → { "id": "staging", "state": "running", "content": "Next hunk: ..." }

assistant: call(git, { action: "fetch", id: "staging" })
  → { "id": "staging", "state": "stopped", "result": "Staged 3 hunks." }
```

Today, the `git` tool can only run non-interactive commands.
With the stateful protocol, the tool author builds a `git` tool that uses JP's
infrastructure to manage the interactive subprocess, and the assistant drives
the session through standard tool calls.

### Sub-agents

This protocol also lays the groundwork for sub-agents, where the main assistant
spawns sub-assistants to run tasks concurrently.
From the protocol's perspective, a sub-agent is another stateful tool: it
spawns, produces output over time, accepts input, and eventually stops.
The handle registry, action-based schema, and assistant-driven polling model all
apply directly, though sub-agents themselves are outside the scope of this RFD.

## Design

### `ToolState`

`ToolState` replaces the `Outcome` enum and represents a tool's state at any
point in its lifecycle:

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

Every state carries optional string content.
The tool author controls its format: plain text, JSON, XML, or combined stdout
and stderr.

`Running` can include output from the first response onward, including startup
output such as a shell prompt, editor screen, or welcome message, which avoids
an extra round-trip to fetch initial content.

`Stopped` carries `Result<String, ToolError>`: either the final output string,
or an error message with an optional trace and a `transient` flag indicating
whether the call can be retried (for example, after a network error).

#### Why `content` is a `String`

Structured data that JP acts on (`Question` in `Waiting`, `ToolError` in
`Stopped`) lives in dedicated typed fields on each variant, not inside
`content`.
The `content` field is opaque output for the assistant, and JP passes it through
without parsing it.

Future variants can add typed fields without changing `content`'s type.
For example, a sub-agent tool that surfaces an inquiry returns `Waiting {
question, .. }` with the `Question` in the typed field.
The sub-agent serializes its internal state as JSON over the wire, and JP
deserializes it into `ToolState`, matching how local tools write `Outcome` JSON
to stdout.

Using `serde_json::Value` or a generic `ToolState<T: Serialize>` was rejected:
`Value` forces every consumer to parse, and a type parameter infects every
containing type.
In practice, `content` is almost always a string.
If sub-agents need a richer wire format later, a future RFD can extend the
variants or change `content`'s type.

### State transitions

```txt
Running → Stopped
   ↓
 Waiting → Running (after Apply)
   ↓
 Stopped (if aborted while waiting)
```

A stateful tool starts in `Running`, indicating that the tool is active and may
have produced output.
`Waiting` pauses execution until JP or the assistant answers a structured
`Question`, mapping to the existing inquiry system.
`Stopped` is terminal: the tool has exited with either `Ok(content)` or
`Err(...)`.

### `ToolCommand`

When the assistant calls a tool, JP parses the arguments and produces a
`ToolCommand`:

```rust
pub enum ToolCommand {
    /// Spawn a new tool process with an assistant-chosen handle ID.
    Spawn { id: String, args: Map<String, Value> },

    /// Read the current state of a running tool.
    Fetch { id: String },

    /// Send input to a running or waiting tool.
    Apply { id: String, input: Value },

    /// Terminate a running tool.
    Abort { id: String },
}
```

For one-shot tools (no `action` field in arguments), JP synthesizes a `Spawn`
followed by a blocking `Fetch` loop until the tool reaches `Stopped`, hiding
intermediate states from the assistant.

For stateful tools (arguments include an `action` field), JP dispatches the
command directly and returns the resulting `ToolState` to the assistant.

#### Routing `Apply`

`Apply { id, input }` routes `input` based on the handle's current state:

1. **`Running` handle**: JP writes `input` (typically a string, such as `"y\n"`
   for `git add --patch`) to the tool's stdin or input mechanism.
2. **`Waiting` handle**: JP delivers `input` (a boolean, string, or other typed
   value) as the answer to the pending `Question`, matching the data produced by
   the inquiry system or user prompt.
   JP extracts `question.id` from the handle's `Waiting { question, .. }` state
   and records `accumulated_answers[question.id] = input`.

Because a handle is in one state at a time and has at most one pending question,
the caller (the assistant or JP's one-shot wrapper) does not pass a question ID
in `Apply`.
JP derives it from the handle's state, preserving the correlation that the
inquiry system provides through explicit `InquiryId` matching.

For one-shot shell tools in `Waiting`, `Apply` re-executes the command with the
accumulated answers (matching existing `NeedsInput` behavior).
For stateful tools, `Apply` delivers the input to the running process.

#### Per-tool action sets

A stateful tool declares the actions it supports, and only those actions appear
in the schema exposed to the assistant:

| Tool                  | Actions                            | Why                                 |
| --------------------- | ---------------------------------- | ----------------------------------- |
| `git` (interactive)   | `spawn`, `fetch`, `apply`, `abort` | Needs input for interactive staging |
| `cargo_check` (async) | `spawn`, `fetch`, `abort`          | No stdin, just poll for completion  |
| Background task       | `spawn`, `fetch`                   | Fire and forget, cannot be stopped  |

Because unsupported actions are omitted from the schema, the assistant cannot
call them; JP never has to handle an `apply` sent to a tool that takes no input.

#### Mapping from current `Outcome`

| Current `Outcome`                     | New `ToolState`                     |
| ------------------------------------- | ----------------------------------- |
| `Success { content }`                 | `Stopped { result: Ok(_) }`         |
| `Error { message, trace, transient }` | `Stopped { Result: Err(_) }`        |
| `NeedsInput { question }`             | `Waiting { content: "", question }` |

### Stateful tool schemas

A stateful tool exposes `action` and `id` in its JSON schema alongside its
tool-specific parameters.
The schema uses `oneOf` with a sub-schema for each supported action.

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
        "id": {
          "type": "string",
          "description": "Handle ID for this tool instance. Must be unique across active handles."
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
        "id",
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

Example schema for `cargo_check` (supports `spawn`, `fetch`, and `abort`,
without `apply` because it takes no input):

```json
{
  "type": "object",
  "oneOf": [
    {
      "properties": {
        "action": {
          "const": "spawn"
        },
        "id": {
          "type": "string",
          "description": "Handle ID for this tool instance. Must be unique across active handles."
        },
        "package": {
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

The tool author defines the `spawn` variant's parameters and the supported
action set; `fetch`, `apply`, and `abort` follow a fixed pattern.
For Rust tools, JP's SDK generates the full schema from the tool's definition.
Tools in other languages generate the schema directly.

### Handle registry

JP maintains a handle registry in the query execution loop that maps handle IDs
to running tool instances across tool-call rounds within a turn.

```rust
struct HandleRegistry {
    handles: HashMap<String, ToolHandle>,
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

The assistant chooses the handle ID via the required `id` parameter on `spawn`,
using descriptive names (`check`, `staging`, `build_release`) that stay readable
in conversation history.
Unlike JP-generated sequential IDs (`h_1`, `h_2`), assistant-chosen IDs are
known at call time without waiting for JP's response.
JP enforces uniqueness across active handles and returns an error if a `spawn`
ID collides.

When the assistant calls `tool(action: "fetch", id: "staging")`, JP looks up
`staging` in the registry, calls the tool's fetch handler, and returns the
result.

### Handle lifecycle

Handles are created when a tool returns a non-terminal state and destroyed when:

- The tool reaches `Stopped` (natural completion)

- The assistant or user sends `Abort`

- The conversation turn ends (all remaining handles are aborted)

  > [!TIP]
  > [RFD 037] extends turn-end behavior with configurable per-tool policies
  > (`inquire`, `await`, `abort`).

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

`StreamEventHandler.handle_tool_call` runs this loop transparently, so the
assistant sees a standard one-shot response.
When a tool returns `Waiting`, JP collects the answer via a user prompt or the
inquiry system and sends `Apply` to continue execution, replacing the
`NeedsInput` re-execution loop.

For a shell tool that exits immediately, JP spawns the process, parses its
stdout as `Outcome`, converts that to `ToolState::Stopped`, and destroys the
short-lived handle before returning.

### Detecting stateful vs. one-shot calls

JP distinguishes stateful and one-shot calls by the **invocation**, not by the
tool's return value:

- **`action` field present (stateful)**: JP dispatches the `ToolCommand`
  directly.
  A `spawn` call registers a handle; subsequent `fetch`, `apply`, and `abort`
  calls reference that handle by ID.
- **No `action` field (one-shot)**: JP wraps the call in the internal
  spawn-and-fetch loop without registering a persistent handle.
  If the tool returns `Waiting` (for example, `fs_modify_file` asking about a
  dirty file), JP resolves the question via the inquiry or prompt path and
  re-executes the tool with accumulated answers.

Dispatching on invocation is necessary because a one-shot shell tool that
returns `Waiting` has already exited and has no live process to receive `Apply`.
In the one-shot path, `Waiting` means "re-execute with answers" (matching
`NeedsInput`); in the stateful path, it means "the process is paused, deliver
input via `Apply`."

Existing tools such as `fs_modify_file` require no changes.
Because their schemas omit `action`, the assistant never sends an `action`
field, and JP runs them through the one-shot path.

To expose `action` and `id` in a tool's JSON schema, the tool's configuration or
definition must declare stateful support, for example through:

- A flag in the tool config (`stateful = true`)
- An `action` parameter in the tool definition
- A programmatic declaration on built-in tools

The exact declaration mechanism will be resolved during implementation.

### Integration with existing tool system

`StreamEventHandler.handle_tool_call` currently:

1. Resolves the tool config and definition
2. Handles run mode (ask/unattended/edit/skip)
3. Calls `ToolDefinition::call` in a loop (retrying on `NeedsInput`)
4. Collects `ToolCallResponse`

With the stateful protocol, step 3 changes:

- **One-shot tools**: Same loop, modeled internally as spawn → fetch →
  stopped, with the `NeedsInput` retry loop mapped to `Waiting` → `Apply`.
- **Stateful tools**: No loop.
  JP dispatches the `ToolCommand`, returns the `ToolState` to the assistant, and
  lets the assistant drive subsequent calls.

The `ToolCallResponse` sent to the assistant for stateful tools includes the
handle ID and state:

```rust
ToolCallResponse {
    id: call.id,
    result: Ok(json!({
        "id": "staging",
        "state": "running",
        "content": "Stage this hunk? [y/n/...]"
    }).to_string()),
}
```

The conversation event stream (`ToolCallRequest` → `ToolCallResponse`) is
unchanged.
Each `action` call is a separate tool call round from the assistant's
perspective.

### Interaction with the inquiry system

The inquiry system handles `NeedsInput` with `QuestionTarget::Assistant` by
making a structured LLM call to get the answer and then re-executing the tool.
Under the stateful protocol, the inquiry system produces the answer the same way
and delivers it via `Apply`:

1. The tool returns `Waiting { question, .. }` (with `target: Assistant` in
   config).
2. JP's inquiry system makes the structured LLM call and extracts the answer.
3. JP sends `Apply { input: answer }` to the tool handle.
4. For one-shot shell tools, `Apply` re-executes the command with accumulated
   answers (since the process has already exited).
5. For stateful tools, `Apply` delivers the answer to the running process.

When the assistant drives a stateful tool directly without the inquiry system:

1. The tool returns `Waiting { question, .. }`, which JP forwards to the
   assistant in the `ToolCallResponse`.
2. The assistant calls `tool(action: "apply", id: "staging", input: true)`.
3. JP sends `Apply { input: true }` to the handle.
4. The tool receives the answer and continues.

Both paths use `Apply`, and the handle routes the input based on its state
(stdin write vs. question answer).

### `ToolState` in `jp_tool`

`Outcome` is the public return type in `jp_tool`.
Migrating to `ToolState` proceeds in four steps:

1. Add `ToolState` alongside `Outcome` in `jp_tool`.
2. Implement `From<Outcome> for ToolState` for backward compatibility.
3. Update internal code to use `ToolState`.
4. Deprecate `Outcome` while continuing to accept its JSON format from shell
   tools.

<!-- end list -->

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

**Complexity.** The state machine adds an internal abstraction layer over a
simple call-and-return model, with no user-visible benefit for the majority of
tools, which are one-shot.

**Handle management.** Long-lived handles introduce state that must be tracked,
preserved across tool-call rounds, and cleaned up on error.
JP's tool system currently holds no cross-round state beyond
`TurnState.pending_tool_call_questions`.

**Schema complexity.** The `oneOf` schema for stateful tools is more complex
than a flat parameter list.
Some LLM providers handle `oneOf` schemas poorly or not at all, which may
require provider-specific schema transformations.

**Two execution paths.** Despite the unified model, one-shot and stateful tools
still take different code paths (wrap-and-loop vs. direct dispatch).
The abstraction unifies the types, not the implementation.

## Alternatives

### Keep tools strictly one-shot, add separate "session" tools

An earlier draft of this RFD exposed PTY sessions as generic `terminal_*` tools,
where the assistant would call `terminal_start("git add --patch")` to launch any
interactive program.

**Rejected because:** It exposes the wrong abstraction.
The assistant should call domain tools (`git`, `editor`), not generic terminal
tools.
The stateful protocol lets each tool define its own commands and parameters
while reusing JP's handle management infrastructure.

### Make statefulness a tool config property only

Require `stateful = true` in the tool configuration as the sole way to declare
stateful support, rather than also allowing tool definitions and built-in tools
to declare stateful schemas directly or inferring statefulness from the tool's
return value.

**Rejected because:** While a tool must declare stateful support so its schema
includes `action` and `id`, making configuration the only mechanism adds
unnecessary configuration burden.
In the proposed design, the schema declaration drives both schema generation and
runtime dispatch (presence of `action` in arguments).

### Separate `ToolState` from `Outcome` entirely

Make `ToolState` and `Outcome` independent types with no `From<Outcome> for
ToolState` conversion.

**Rejected because:** It would break every existing tool immediately.
The conversion keeps existing tools that emit `Outcome` JSON working without
changes.

## Non-Goals

- **PTY / terminal emulation.** This RFD covers the protocol and handle
  management; subprocess management (PTY, terminal emulator, screen buffer) is
  covered in a follow-up RFD.

- **Interactive tool SDK.** A convenience SDK (`jp_tool::interactive`) for
  building stateful tools in Rust is a follow-up; this RFD defines the protocol
  that SDK targets.

- **Parallel stateful tools.** Multiple handles can coexist, but coordinating
  between them (synchronizing across handles or piping output from one to
  another) is out of scope.

  > [!TIP]
  > [RFD 037] introduces an `await` built-in tool for cross-handle
  > synchronization.

- **Persistent handles across JP invocations.** Handles are scoped to the JP
  process and destroyed when JP exits.

## Risks and Open Questions

### Schema generation for stateful tools

LLM providers do not universally support the `oneOf` schema pattern for `action`
variants.
Google's Gemini supports `anyOf`, but [strictly requires it to be the only field
in the schema object][gemini-anyof], with no sibling properties alongside it.
Other providers have their own restrictions.
JP may need to apply per-provider schema transformations or fall back to a flat
schema with optional fields and explain the action protocol in `description`.
Each provider needs validation.

### Handle cleanup guarantees

If JP is killed with `SIGKILL`, handles are not cleaned up and child processes
become orphans.
The current subprocess model shares this risk, but stateful tools are more
likely to run long-lived processes that leave behind visible state such as lock
files or half-written files.
We should document this limitation and consider a startup cleanup mechanism (for
example, a `.jp/handles.lock` file).

### Token cost of stateful interactions

Each `fetch` or `apply` round is a full LLM tool-call round-trip, so a 20-step
interactive session adds 20 rounds of input and output tokens.
For tools that return full terminal screens, this cost adds up quickly.
Token optimization (for example, returning diffs instead of full screens) is
left to individual tool implementations.

### Interaction with tool permission system

`RunMode::Ask` currently prompts the user before each tool call.
Prompting before every `fetch` and `apply` would make interactive workflows
unusable.
JP should prompt on `spawn` and run `fetch`, `apply`, and `abort` unattended by
default, with permissions configurable per action rather than only per tool.

### Proactive delivery of stopped handles

When a stateful tool reaches `Stopped` in the background (for example, `cargo
check` finishes while the assistant is doing other work), JP cannot inject a
`ToolCallResponse` without a matching `ToolCallRequest`.
Three options were considered:

- **Require the assistant to poll.** Simple, but the assistant must remember to
  `fetch` every handle it spawned.
- **Fabricate a `ToolCallRequest`/`ToolCallResponse` pair.** Violates the event
  model contract.
- **Deliver at turn end.** The assistant has already finished its response, so
  the result is only available in the next turn.

This RFD uses **assistant-driven polling**: the assistant checks on its own
handles, and JP does not push results proactively.

A follow-up design can add a general-purpose **system message queue** that
piggybacks notifications onto existing messages, informing the assistant of
handle state changes (as well as MCP disconnections or workspace changes) on a
per-tool basis without fabricating events.

> [!TIP]
> [RFD 011] introduces the system message queue.
> [RFD 037] introduces the `await` tool for explicit synchronization and
> configurable per-tool notifications and turn-end behavior.

### Backward compatibility of `ToolState` serialization

`ToolState` JSON (`"type": "spawned"`, `"running"`, `"waiting"`, `"stopped"`)
differs from `Outcome` JSON (`"type": "success"`, `"error"`, `"needs_input"`).
JP accepts both formats from tool stdout by inspecting the `type` field:
`spawned`, `running`, `waiting`, and `stopped` parse as `ToolState`, while
`success`, `error`, and `needs_input` parse as `Outcome` and convert via
`From<Outcome>`.

## Implementation Plan

### Phase 1: `ToolState` type in `jp_tool`

Add `ToolState` and `ToolError` alongside `Outcome` in `jp_tool`, implement
`From<Outcome> for ToolState`, and add backward-compatible deserialization that
accepts both formats, with unit tests for conversion and serialization.

This phase introduces no behavioral changes and can be merged independently.

### Phase 2: `ToolCommand` type and handle registry

Add `ToolCommand` to `jp_tool`, create `HandleRegistry` (in `jp_tool::handle` or
`jp_cli`), and define the `AnyToolHandle` trait for tool implementations.

This phase introduces no behavioral changes and can be merged independently.

### Phase 3: One-shot tool wrapping

Refactor `StreamEventHandler.handle_tool_call` to run one-shot calls through the
spawn → fetch → apply cycle instead of the `NeedsInput` retry loop.
Change `ToolDefinition::call` to return `ToolState` instead of
`ToolCallResponse`, and convert `ToolState::Stopped` to `ToolCallResponse` in
the wrapping loop.

This phase must preserve existing behavior across the tool-call test suite.
Depends on Phases 1 and 2.

### Phase 4: Stateful tool dispatch

Parse `action` and `id` from tool-call arguments.
When `action` is present, dispatch the `ToolCommand` to `HandleRegistry` instead
of running the one-shot loop, and return `ToolState` as the tool-call response
content.

Depends on Phase 3.

### Phase 5: Schema generation for stateful tools

Generate the `oneOf` schema from a tool's declared action set, integrate it with
`ToolDefinition::to_parameters_schema`, and handle provider-specific schema
limitations.

Depends on Phase 4 and can be iterated on independently.

## References

- [RFD 028: Structured Inquiry System][RFD 028]: the inquiry system that handles
  `Waiting` with `QuestionTarget::Assistant`.
- [Query Stream Pipeline]: the turn loop and tool execution flow that this RFD
  modifies.
- [\#392]: PTY-based end-to-end CLI testing (related infrastructure).
- [interminai]: prior art for PTY-based tool interaction.

[Query Stream Pipeline]: ../wiki/00dxyb6-query-stream-pipeline-architecture.md
[RFD 010]: 010-pty-infrastructure-and-interactive-tool-sdk.md
[RFD 011]: 011-system-notification-queue.md
[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 037]: 037-await-tool-for-stateful-handle-synchronization.md
[\#392]: https://github.com/dcdpr/jp/issues/392
[`inquiry` system]: ./005-first-class-inquiry-events.md
[gemini-anyof]: https://github.com/anomalyco/opencode/issues/14509
[interminai]: https://github.com/mstsirkin/interminai
