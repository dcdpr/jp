# RFD 018: Prompts as Inquiry Events

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-01
- **Required by**: [RFD 049]
- **Extends**: [RFD 082]
- **Summary**: Every prompt becomes an inquiry event written before it is shown;
  frontends draw and answer prompts from those events.

## Summary

Every prompt JP shows during a turn becomes an `InquiryRequest` event before
anything is drawn: permission to run a tool, delivery of a tool's result, and a
tool's own question.
The coordinator asks by writing the request, then waits for the matching
`InquiryResponse`.
The terminal prompter, config policy, and the inquiry backend are answerers:
they read the pending request and write the answer back.

## Motivation

JP has three kinds of prompts during tool execution:

1. **Permission**: "Run local `fs_modify_file` tool?", asked by
   `ToolPrompter::prompt_permission`.
2. **Result delivery**: "Deliver `fs_read_file` result to assistant?", asked by
   `ToolPrompter::prompt_result_confirmation`.
3. **Tool question**: "Overwrite existing file?", from `Outcome::NeedsInput`.

[RFD 082] records every tool question as an `InquiryRequest`/`InquiryResponse`
pair.
Permission and result prompts are outside its scope: they exist only as a
function call that blocks on the terminal.
Even for tool questions, the prompter draws from in-memory state, and the event
is a record written next to it.

Everything outside the running prompt is left without the information:

- **Other processes can't see that a prompt is waiting.** With a permission
  prompt on screen, the conversation on disk looks like a turn that is still
  streaming.
  A workspace dashboard shows `running` for a turn that is waiting on the user.
- **[RFD 023] can't resume it.** A permission prompt the process died on leaves
  no trace, so the turn reads as `interrupted (pending tool execution)`, not
  `waiting-for-input`.
- **[RFD 027] can't route it.** A server that asks and a client that answers
  need the question as data between them. 027 already derives
  `waiting-for-input` from the stream, not from process state.
- **[RFD 092] can't show it again.** Its `[b] back` returns to a prompt after an
  interrupt menu, so every prompt has to exist as queued data.
- **[RFD 049] routes by prompt kind** and needs one type to route on.

Each needs the same thing: the prompt exists as data in the conversation before
any frontend draws it.

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

1. The coordinator decides a prompt is needed, writes the `InquiryRequest`, and
   flushes it.
   Nothing is drawn before the flush.
2. Routing hands the request to exactly one answerer.
3. The answerer returns an answer, and the coordinator writes the
   `InquiryResponse` and continues.

The terminal prompter's input is the `InquiryRequest`, the conversation stream,
and config.
It does not receive the executor, `PermissionInfo`, or any other in-memory
state.
That restriction is what the design buys: a prompt the terminal can draw from
the event is a prompt any frontend can draw, and one a resumed process can ask
again.

Interactions that answer nothing stay in the frontend.
The permission prompt's `p` prints the arguments as JSON and returns to the same
prompt; it writes no event.

### The `Prompt` type

`InquiryRequest` gains a `prompt` field that says what is being asked:

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

The variants carry references, not presentation.
The tool name and arguments come from the referenced `ToolCallRequest` event,
and the source label ("local", "mcp") and styling come from config when the
prompt is drawn.
`question` holds a generic form, text and answer type, so a frontend that
doesn't know a kind can still show and answer it.

`InquirySource` gains a `Jp` variant.
Permission and result prompts are asked by JP, not by the tool or the assistant.
Tool questions keep 082's attribution.

A tool can only produce `Outcome::NeedsInput`, which becomes `ToolQuestion`.
`RunTool` and `DeliverToolResult` need a tool call ID that only the coordinator
holds, so a tool cannot ask one.

Inquiry IDs keep 082's format, `<tool_call_id>.<question_id>.<attempt>`, with
`run_tool` and `deliver_tool_result` as reserved question IDs.
`QuestionId` validation rejects those two names from tools, so a tool's question
never collides with one of JP's.

### Answers

The generic `question` for the two JP prompts is a select: `run` or `skip`, and
`deliver` or `skip`.
A kind-aware answerer writes an object with the choice and any payload.
Readers accept a bare option value as `{ "choice": <value> }`.

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

Edits land in the history.
Edited arguments are in the answer, because the `ToolCallRequest` keeps what the
assistant sent.
An edited result is the `ToolCallResponse` itself, so the answer only marks that
it was edited.
`remember` makes the turn's remembered decisions recoverable from the stream,
which [RFD 023] needs to resume a turn.

Cancelling any prompt records `InquiryResponse::Cancelled` with 082's reasons.

### When an inquiry exists

An inquiry exists when the configured mode asks.
Who answers doesn't change that.

| Configuration and situation                 | Inquiry | Answered by                            |
| ------------------------------------------- | ------- | -------------------------------------- |
| `run = "ask"`, user at the terminal         | yes     | terminal prompter                      |
| `run = "ask"`, `Y` or `N` earlier this turn | yes     | policy                                 |
| `run = "ask"`, no terminal                  | yes     | policy (configurable in [RFD 049]) |
| `run = "edit"`                              | yes     | terminal prompter, in the editor       |
| `run = "unattended"` or `"skip"`            | no      | nothing was asked                      |
| `result = "ask"` or `"edit"`                | yes     | terminal prompter or policy            |
| `result = "unattended"` or `"skip"`         | no      | nothing was asked                      |

This follows 082, which records questions answered from config or from an
earlier answer in the same turn.
Tools configured to run without asking produce no inquiry, so the common case
adds no events.

### Pending prompts

The coordinator's `PendingPrompt` queue holds tool questions and result prompts,
and permission prompts bypass it.
Under this design the queue is the turn's unanswered inquiries in stream order,
and permission prompts join it.
The streaming path keeps its ordering: it waits for the permission answer before
it processes further events from the assistant.
One prompt is drawn at a time.

This covers [RFD 092]'s phase 4.
`ConversationStream::pending_inquiries()`, which pairs requests with responses
for tool questions, reports every kind without change.

### Routing

Routing chooses the answerer.
Two properties derive from the variant, as functions over `Prompt` in `jp_cli`:

- `exclusive()`: `RunTool` and `DeliverToolResult` are human-only.
  A `ToolQuestion` is not, unless config says so.
- `config_key()`: `run`, `deliver`, or `tool`, the keys [RFD 049]'s detached
  policy uses.

In this RFD routing keeps today's behavior.
Terminal detection decides between the terminal prompter and policy, and tool
questions keep their `target` config.

## Drawbacks

- **A real refactor.** The permission path is a direct call inside the streaming
  loop (`resolve_tool_call_decision` calls `prompt_permission`).
  Making it write-then-wait touches the coordinator, the prompter, and the turn
  loop.
  [RFD 092] names the same cost.
- **More events.** Every asked permission and result prompt adds a
  request/response pair, visible in event counts and to anything that walks the
  stream.
- **Format growth.** `InquiryRequest` gains a field and `InquirySource` a
  variant.
- **Lookups to draw.** The prompter finds the `ToolCallRequest` by ID, where it
  is handed the arguments today.

## Alternatives

**Flag an open prompt in the conversation lock file.** `jp` writes "a prompt is
open since T" into the lock holder's info while any prompt is on screen.
It is cheap, covers every prompt, and changes no conversation data.
Rejected: it describes the process, not the conversation.
The flag disappears when the process exits, which [RFD 027] plans for, and it
makes no prompt resumable, re-showable, or answerable anywhere else.

**Record each terminal prompt as it is shown.** The prompter stays in charge and
writes an event for every prompt it draws.
Rejected: the terminal remains the source of truth and the event is a copy that
can drift from what was drawn.
Nothing can answer or re-show a prompt from such a record, because the record
never drove the prompt.

**Keep `Prompt` in memory only**, as this RFD first proposed.
Routing gets one type, but nothing outside the running process sees a prompt.

## Non-Goals

- **Answering from another process.** A dashboard or client that writes an
  answer needs the process holding the conversation to accept it.
  That is [RFD 027]'s IPC.
- **Detached prompt policy.** Configurable policies are [RFD 049].
- **Interrupt menus and `[b] back`.** [RFD 092] builds on the queue described
  here.
- **Showing inquiry events** in `jp conversation print`.
- **New prompt kinds.** The apply prompt from [RFD 078] becomes a `Prompt`
  variant when it lands; this RFD does not specify it.

## Risks and Open Questions

- **Older builds reading newer streams.** Conversation events don't reject
  unknown fields, so older builds ignore `prompt`.
  They do fail on an unknown `InquirySource` tag.
  Phase 1 makes `InquirySource` read unknown tags as `Other`, as 082 did for
  `CancellationReason`, and ships before anything writes `Jp`.
  Builds older than phase 1 still fail on such a stream.
- **Replay after edited arguments.** The assistant sees the arguments it sent
  next to a result produced from the edited ones.
  That is today's behavior; this RFD records it but doesn't change it.
- **Delivery prompts ask about a result that isn't on disk.** Asking
  `DeliverToolResult` again after a restart means running the tool again, as
  [RFD 023] does for tool questions.
- **Who answered.** Neither 082 nor this RFD records the answerer, so a policy
  answer and a user's `y` look the same on disk.
  If that distinction is needed, it is an additive field on `InquiryResponse`.
- **Custom argument formatters.** `format = "ask"` defers a custom formatter
  until after approval.
  Drawing a permission prompt again must not run it early.

## Implementation Plan

### Phase 1: Data model

Add `Prompt` and the `prompt` field on `InquiryRequest`, the `Jp` source,
tolerant `InquirySource` deserialization, and the two reserved question IDs.
Nothing writes the new shapes yet.
Mergeable on its own.

### Phase 2: Permission prompts as inquiries

The coordinator writes and flushes a `RunTool` request before any prompt is
drawn, routes it, and writes the response.
The terminal prompter draws from the request and the stream.
Remembered and non-interactive answers are recorded.
Permission prompts join the pending queue, which covers [RFD 092]'s phase 4.
Depends on phase 1.

### Phase 3: Result prompts as inquiries

The same for `DeliverToolResult`.
Depends on phase 2.

### Phase 4: Tool questions drawn from events

Tool questions already produce events.
The prompter switches from the in-memory `Question` to the request, and the
flush that tool questions do before prompting becomes the general path.
`PendingPrompt` is replaced by the stream's pending inquiries.
Depends on phase 2.

## References

- [RFD 005] and [RFD 082]: inquiry events for tool questions, extended here to
  every prompt.
- [RFD 023], [RFD 027], [RFD 049], [RFD 092]: consumers of the events and the
  type described here.
- [RFD 028]: the inquiry backend, one of the answerers.
- [RFD 078]: a future prompt kind.

[RFD 005]: 005-first-class-inquiry-events.md
[RFD 023]: 023-resumable-conversation-turns.md
[RFD 027]: 027-client-server-query-architecture.md
[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 078]: 078-tool-config-mutation.md
[RFD 082]: 082-unified-inquiry-event-recording.md
[RFD 092]: 092-predictable-and-responsive-interrupt-escalation.md
