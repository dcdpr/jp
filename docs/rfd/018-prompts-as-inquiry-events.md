# RFD 018: Prompts as Inquiry Events

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-01
- **Required by**: [RFD 049]
- **Extends**: [RFD 082]
- **Summary**: Records every prompt as an inquiry event before display so
  frontends render and answer prompts from the stream.

## Summary

This RFD represents every prompt during a turn (tool execution permission, tool
result delivery, and tool questions) as an `InquiryRequest` event flushed to the
conversation stream before any frontend renders it.
`ToolCoordinator` writes and flushes the `InquiryRequest`, routes it to an
answerer (the terminal prompter, config policy, or the inquiry backend), and
writes the resulting `InquiryResponse` before continuing the tool call.

## Motivation

JP has three kinds of prompts during tool execution:

1. **Permission**: "Run local `fs_modify_file` tool?", asked by
   `ToolPrompter::prompt_permission`.
2. **Result delivery**: "Deliver `fs_read_file` result to assistant?", asked by
   `ToolPrompter::prompt_result_confirmation`.
3. **Tool question**: "Overwrite existing file?", from `Outcome::NeedsInput`.

[RFD 082] records every tool question as an `InquiryRequest`/`InquiryResponse`
pair, while permission and result-delivery prompts exist only as synchronous
function calls that block on the terminal.
Even for tool questions, the prompter renders from in-memory state and writes
the inquiry event alongside it.

Because prompts do not exist as events before they are displayed, nothing
outside the active prompt call can inspect or replay them:

- **External processes cannot see pending prompts.** While a permission prompt
  is on screen, the persisted conversation stream still looks like an actively
  streaming turn, so a workspace dashboard reports `running` instead of waiting
  on the user.
- **[RFD 023] cannot resume interrupted permission prompts.** If the process
  exits while waiting at a permission prompt, no event is on disk, so the turn
  appears as `interrupted (pending tool execution)` instead of
  `waiting-for-input`.
- **[RFD 027] cannot forward prompts across processes.** A server that requests
  input and a client that answers it need the prompt serialized between them;
  [RFD 027] already derives `waiting-for-input` from the conversation stream
  rather than in-memory process state.
- **[RFD 092] cannot re-display a prompt after an interrupt.** Selecting `[b]
  back` in an interrupt menu returns to the active prompt, which requires every
  prompt to exist as queued data.
- **[RFD 049] cannot dispatch across prompt kinds.** Detached prompt policies
  route by prompt kind and require a single type covering all three prompts.

Each of these features requires the prompt to exist as an event in the
conversation stream before any frontend renders it.

## Design

### Ask, then answer

```text
coordinator                  conversation stream               answerer
    │ write InquiryRequest ──────► flushed to disk
    │                                    ├──► policy: config or remembered answer
    │                                    ├──► terminal prompter: draws the event
    │                                    └──► inquiry backend: the assistant
    │ ◄──────────── InquiryResponse ◄────┘
    ▼ continue the tool call
```

1. When a prompt is needed, the coordinator writes the `InquiryRequest` to the
   conversation stream and flushes it before any frontend renders the prompt.
2. Routing dispatches the request to a single answerer.
3. The answerer returns an answer, and the coordinator writes the matching
   `InquiryResponse` and continues the tool call.

The terminal prompter receives only the `InquiryRequest`, the conversation
stream, and config; it does not take the executor, `PermissionInfo`, or other
in-memory state.
This constraint guarantees that the event is self-contained: any prompt the
terminal prompter can render from the `InquiryRequest` and stream can also be
rendered by another frontend or re-asked by a resumed process.

Interactions that do not produce an answer remain local to the frontend.
For example, pressing `p` at a permission prompt prints the tool arguments as
JSON and re-displays the prompt without writing an event.

### The `Prompt` type

`InquiryRequest` gains a `prompt` field that identifies the prompt kind:

```rust
pub struct InquiryRequest {
    pub id: InquiryId,
    pub source: InquirySource,
    /// What is being asked.
    /// Absent in streams written before this RFD; read as `ToolQuestion`.
    #[serde(default)]
    pub prompt: Prompt,
    /// The question in a form any frontend can draw and answer.
    pub question: InquiryQuestion,
}

#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Prompt {
    /// Should this tool call run?
    RunTool { tool_call_id: String },
    /// Should this tool call's result be delivered to the assistant?
    DeliverToolResult { tool_call_id: String },
    /// A question the tool asked while running.
    #[default]
    ToolQuestion,
}
```

The `Prompt` variants carry event references rather than presentation data.
When rendering a prompt, the frontend looks up the tool name and arguments on
the referenced `ToolCallRequest` event and reads the source label (`local`,
`mcp`) and styling from config.
`question` stores a generic fallback (prompt text and answer type) so a frontend
without specialized rendering for a `Prompt` variant can still display and
answer it.

`InquirySource` gains a `Jp` variant because permission and result-delivery
prompts originate from JP rather than from a tool or the assistant.
Tool questions keep their [RFD 082] source attribution.

Tools can only return `Outcome::NeedsInput`, which the coordinator maps to
`Prompt::ToolQuestion`.
Constructing `RunTool` or `DeliverToolResult` requires the `tool_call_id` held
by the coordinator, so a tool cannot emit either prompt.

Inquiry IDs keep [RFD 082]'s `<tool_call_id>.<question_id>.<attempt>` format,
with `run_tool` and `deliver_tool_result` reserved as question IDs.
`QuestionId` validation rejects both reserved names when a tool emits a
question, preventing collisions with JP's own prompts.

### Answers

For JP's two built-in prompts, the generic `question` is a select prompt with
options `run` or `skip` for `RunTool`, and `deliver` or `skip` for
`DeliverToolResult`.
A prompt-aware answerer returns a JSON object containing `choice` and any
prompt-specific fields, while readers also accept a bare option string
`"<value>"` as shorthand for `{ "choice": "<value>" }`.

| Terminal key | `RunTool` answer                          |
| ------------ | ----------------------------------------- |
| `y`          | `{ "choice": "run" }`                     |
| `Y`          | `{ "choice": "run", "remember": true }`   |
| `n`          | `{ "choice": "skip" }`                    |
| `N`          | `{ "choice": "skip", "remember": true }`  |
| `r`          | `{ "choice": "skip", "reason": "…" }`     |
| `e`          | `{ "choice": "run", "arguments": { … } }` |
| `p`          | no answer; display only                   |

| Terminal key | `DeliverToolResult` answer                |
| ------------ | ----------------------------------------- |
| `y`          | `{ "choice": "deliver" }`                 |
| `n`          | `{ "choice": "skip" }`                    |
| `e`          | `{ "choice": "deliver", "edited": true }` |

User edits are recorded in the conversation stream.
For `RunTool`, `ToolCallRequest` preserves the arguments the assistant
originally sent, so edited arguments are stored in the `InquiryResponse`
payload.
For `DeliverToolResult`, the edited output is written directly to
`ToolCallResponse`, so the `InquiryResponse` only records `"edited": true`.
Recording `"remember": true` persists in-turn remembered decisions in the stream
so [RFD 023] can reconstruct them when resuming an interrupted turn.

Cancelling any prompt writes `InquiryResponse::Cancelled` using the cancellation
reasons defined in [RFD 082].

### When an inquiry exists

Whether JP records an inquiry depends on the tool's configured `run` or `result`
mode, not on which answerer resolves it:

| Configuration and situation                 | Inquiry | Answered by                            |
| ------------------------------------------- | ------- | -------------------------------------- |
| `run = "ask"`, user at the terminal         | yes     | terminal prompter                      |
| `run = "ask"`, `Y` or `N` earlier this turn | yes     | policy                                 |
| `run = "ask"`, no terminal                  | yes     | policy (configurable in [RFD 049]) |
| `run = "edit"`                              | yes     | terminal prompter, in the editor       |
| `run = "unattended"` or `"skip"`            | no      | nothing was asked                      |
| `result = "ask"` or `"edit"`                | yes     | terminal prompter or policy            |
| `result = "unattended"` or `"skip"`         | no      | nothing was asked                      |

This matches [RFD 082], which records tool questions even when they are answered
from configuration or from a remembered answer earlier in the turn.
Tools configured with `unattended` or `skip` do not ask a question and emit no
inquiry events.

### Pending prompts

Instead of a separate `PendingPrompt` queue that holds only tool questions and
result-delivery prompts while permission prompts bypass it, the pending queue
becomes the turn's unanswered `InquiryRequest` events in stream order, including
permission prompts.
The streaming loop preserves its ordering guarantee by awaiting the permission
`InquiryResponse` before processing subsequent events from the assistant, and
only one prompt is rendered at a time.

Unifying the queue satisfies Phase 4 of [RFD 092].
Because `ConversationStream::pending_inquiries()` already pairs `InquiryRequest`
and `InquiryResponse` events by ID, it reports unanswered prompts of every kind
without modification.

### Routing

Routing selects which answerer handles a pending `InquiryRequest`.
`Prompt` exposes two helper methods in `jp_cli`:

- `exclusive()`: returns `true` for `RunTool` and `DeliverToolResult` (which
  require a human unless detached policy overrides them) and `false` for
  `ToolQuestion` (unless overridden by per-question configuration).
- `config_key()`: returns `"run"`, `"deliver"`, or `"tool"`, matching the
  per-kind detached policy keys in [RFD 049].

This RFD preserves existing routing behavior: terminal detection chooses between
the terminal prompter and policy, and tool questions continue to use their
configured `target`.

## Drawbacks

- **Coordinator and turn-loop refactoring.** Permission prompting is a direct
  synchronous call inside the streaming loop (`resolve_tool_call_decision` calls
  `prompt_permission`).
  Converting it to write an `InquiryRequest` and await an `InquiryResponse`
  requires changes across the coordinator, the prompter, and the turn loop (the
  same refactoring cost noted in [RFD 092]).
- **Larger event streams.** Every permission and result-delivery prompt adds an
  `InquiryRequest`/`InquiryResponse` pair to the conversation stream, increasing
  event counts for stream consumers.
- **Expanded event schema.** `InquiryRequest` gains a `prompt` field and
  `InquirySource` gains a `Jp` variant.
- **Stream lookups during rendering.** Instead of receiving tool arguments
  directly from the caller, the prompter must look up the referenced
  `ToolCallRequest` by ID in the conversation stream.

## Alternatives

**Flag an open prompt in the conversation lock file.** `jp` writes an
open-prompt timestamp into the lock holder's metadata while a prompt is on
screen.
This is cheap, covers every prompt kind, and leaves the conversation stream
unchanged.
Rejected: it describes the process rather than the conversation.
The flag disappears when the process exits, which conflicts with [RFD 027], and
it does not make prompts resumable, re-displayable, or answerable from another
frontend.

**Record each terminal prompt as it is shown.** The prompter remains the source
of truth and writes an event for every prompt it renders.
Rejected: the event is a secondary copy that can drift from what was displayed,
and nothing can answer or re-display a prompt from a passive log.

**Keep `Prompt` in memory only.** Representing prompts with an in-memory
`Prompt` enum gives routing a single dispatch type, but leaves prompts invisible
outside the running process.

## Non-Goals

- **Answering prompts from another process.** Allowing an external dashboard or
  client to submit an `InquiryResponse` to the process holding the conversation
  lock requires the IPC layer in [RFD 027].
- **Detached prompt policy.** Configurable policies for unattended prompts are
  defined in [RFD 049].
- **Interrupt menus and `[b] back`.** [RFD 092] builds on the unified
  pending-inquiry queue defined here.
- **Rendering inquiry events in `jp conversation print`.**
- **Additional prompt kinds.** Future prompts such as the config-apply prompt in
  [RFD 078] will add `Prompt` variants when they land.

## Risks and Open Questions

- **Forward compatibility with older builds.** Conversation event
  deserialization ignores unknown struct fields, so older builds ignore
  `prompt`, but they fail on an unknown `InquirySource` variant.
  Phase 1 adds an `Other` fallback variant to `InquirySource` (matching
  `CancellationReason` in [RFD 082]) and ships before any code writes
  `InquirySource::Jp`.
  Builds predating Phase 1 will still fail to deserialize streams containing
  `Jp`.
- **Stream replay with edited arguments.** When a user edits tool arguments at a
  permission prompt, the assistant still sees its original `ToolCallRequest`
  paired with a `ToolCallResponse` produced from the edited arguments.
  This RFD records the edited arguments in the `InquiryResponse` without
  changing what is sent to the assistant.
- **Resuming an interrupted result-delivery prompt.** Because
  `DeliverToolResult` is asked before `ToolCallResponse` is written to disk,
  resuming an interrupted `DeliverToolResult` prompt after a process restart
  requires re-executing the tool, matching how [RFD 023] handles interrupted
  tool questions.
- **Answerer provenance.** Neither [RFD 082] nor this RFD records which answerer
  produced an `InquiryResponse`, so a policy auto-approval and an interactive
  `y` serialize identically.
  If auditing requires that distinction, `InquiryResponse` can gain an additive
  provenance field.
- **Custom argument formatters.** Setting `format = "ask"` defers running a
  custom parameter formatter until the user approves the tool call.
  Re-rendering a permission prompt from its `InquiryRequest` must preserve that
  deferral and not invoke the formatter before approval.

## Implementation Plan

### Phase 1: Data model

Add `Prompt`, the `prompt` field on `InquiryRequest`, `InquirySource::Jp` with
an `Other` deserialization fallback, and the reserved `run_tool` and
`deliver_tool_result` question IDs.
No code writes the new variants in this phase, so it can merge independently.

### Phase 2: Permission prompts as inquiries

Update the coordinator to write and flush a `Prompt::RunTool` `InquiryRequest`
before rendering a permission prompt, route the request, and record the
resulting `InquiryResponse` (including remembered and non-interactive answers).
Update the terminal prompter to render permission prompts from the
`InquiryRequest` and conversation stream, and move permission prompts into the
pending inquiry queue (covering Phase 4 of [RFD 092]).
Depends on Phase 1.

### Phase 3: Result prompts as inquiries

Apply the same write-route-record flow and stream-based rendering to
`Prompt::DeliverToolResult`.
Depends on Phase 2.

### Phase 4: Tool questions drawn from events

Update the terminal prompter to render `Prompt::ToolQuestion` from its
`InquiryRequest` event instead of the in-memory `Question`, unify pre-prompt
stream flushing across all prompt kinds, and replace `PendingPrompt` with
`ConversationStream::pending_inquiries()`.
Depends on Phase 2.

## References

- [RFD 005] and [RFD 082]: inquiry events for tool questions, extended here to
  every prompt kind.
- [RFD 023], [RFD 027], [RFD 049], [RFD 092]: consumers of the inquiry events
  and `Prompt` type defined here.
- [RFD 028]: the LLM inquiry backend, one of the prompt answerers.
- [RFD 078]: a future config-apply prompt kind.

[RFD 005]: 005-first-class-inquiry-events.md
[RFD 023]: 023-resumable-conversation-turns.md
[RFD 027]: 027-client-server-query-architecture.md
[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 078]: 078-tool-config-mutation.md
[RFD 082]: 082-unified-inquiry-event-recording.md
[RFD 092]: 092-predictable-and-responsive-interrupt-escalation.md
