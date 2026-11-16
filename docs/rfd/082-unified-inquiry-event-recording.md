# RFD 082: Unified Inquiry Event Recording

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-12
- **Extends**: [RFD 005][RFD 005-2]
- **Summary**: Records `InquiryRequest` and `InquiryResponse` events across all
  question routing paths, including `Cancelled` and `Redacted` outcomes.

## Summary

This RFD extends [RFD 005] to record `InquiryRequest` and `InquiryResponse`
events in the persisted conversation stream for every `Outcome::NeedsInput`
round-trip, including user-prompted, cached, and static answers.
It adds `InquiryResponse::Cancelled` for user cancellations and backend errors,
adds `InquiryResponse::Redacted` so secret answers are never written to disk,
and derives `InquirySource` from non-overridable tool metadata.

## Motivation

[RFD 005] explicitly excludes prompter-answered questions: *"Questions answered
via interactive user prompts ... are not recorded as inquiry events."* That
exception was driven by an implementation detail (the prompter path did not
carry a write handle to the conversation stream), not by design.
A question/answer exchange has the same archival value regardless of how it is
routed.
Omitting the prompter path from the stream bakes an implementation gap into the
data model and forces future features that need Q&A visibility (replay,
debugging, sub-agent reasoning trails, conversation viewers) to work around it.

The `InquiryResponse` enum encodes only "answered."
Non-answer outcomes, such as user Ctrl-C and routing-backend errors, have no
on-disk representation, leaving the persisted stream incomplete whenever a tool
question does not complete normally.
In particular, an inquiry-backend failure leaves an `InquiryRequest` without a
matching response.
RFD 082 must close that gap before recording requests across every routing path.

`InquirySource` is constructed from the tool name at the recording site:

```rust
InquirySource::tool(tool_name.clone())
```

This hard-codes provenance to the tool name.
Future tools whose questions come semantically from the assistant rather than
the tool ([RFD 083]'s `ask_user` is the motivating consumer) need to record
those inquiries with `InquirySource::Assistant`.
Making this user-configurable would let a local config bundle relabel any tool's
question as assistant-sourced in the persisted record, defeating the audit
purpose.
Declaring it statically on the tool keeps provenance fixed at compile time
without branching on tool names in coordinator code.
RFD 082 adds the hook; consumers such as [RFD 083] implement it.

## Design

### Stream events across routing paths

Every tool-question round-trip produces an `InquiryRequest` and
`InquiryResponse` pair in the persisted stream between the tool's
`ToolCallRequest` and `ToolCallResponse`.
This pair is recorded uniformly for:

- Questions resolved by the interactive prompter (the user typed an answer), as
  well as questions resolved by the inquiry backend.
- Questions resolved by a cached "remember for turn" answer from earlier in the
  same turn.
- Questions resolved by a static `QuestionConfig.answer` value from user config.
- Questions with `AnswerType::Secret`: the request is recorded, and the response
  is recorded as `Redacted { id }` without the answer payload (see [Secret
  questions](#secret-questions)).
- Questions cancelled by the user (Ctrl-C at the prompt).
- Questions that failed because the routing backend (prompter or inquiry
  backend) returned an error.

Both events in a pair carry the same `id`, which is unique within the turn (see
[Inquiry ID format](#inquiry-id-format)).

### Inquiry ID format

`InquiryRequest.id` correlates an `InquiryRequest` with its matching
`InquiryResponse` and MUST be unique **within the turn**.
IDs do not need to be unique across turns: `TurnMut` permits `tool_call_id`
reuse across turns and across cycles within a turn (see
`same_tool_call_id_across_turns_is_allowed` and
`same_tool_call_id_reused_within_turn_across_cycles` in
`crates/jp_conversation/src/stream/turn_mut_tests.rs`), and `InquiryId` inherits
that cross-turn reuse while remaining unique within a turn.
The format is:

```
<tool_call_id>.<question_id>.<attempt>
```

`attempt` is a 1-indexed counter scoped to `(tool_call_id, question_id)` within
the turn.
The first recording for a `(tool_call_id, question_id)` key in a turn gets
attempt `1`.
Subsequent recordings for the same key within that turn (when the LLM gives an
invalid answer and the tool re-asks, or when a provider like Google Gemini
reuses `tool_call_id` in a later cycle and the same question recurs) increment
the counter to `2`, and so on.

`question_id` MUST NOT contain `.`.
A `QuestionId` newtype in `jp_tool` (wrapping `Question.id`) enforces this in
its `FromStr` and `TryFrom` implementations by rejecting any string containing a
dot.
Validation runs once at the tool-to-JP ingestion boundary: local-tool stdout is
parsed into `QuestionId`, and built-in tools construct questions through the
same validated constructor, making an invalid ID unrepresentable downstream.
`QuestionId` validates only the tool-authored segment, whereas `InquiryId`
(`<tool_call_id>.<question_id>.<attempt>`) is the composite stream-correlation
ID (`attempt` is numeric and therefore dot-free).

When a tool emits a `NeedsInput` with an invalid `question.id`, JP rejects it at
the boundary, logs an `error!` trace with the offending tool and ID, and fails
the tool with a tool-level error response (`tool produced an invalid inquiry:
question id must not contain '.'`).
No separate terminal notice is printed because the tool-level error already
renders through `ErrorStyleConfig`.
Dropping the malformed outcome before constructing any inquiry event preserves
the invariant that every valid `NeedsInput` is recorded: the stream contains the
`ToolCallRequest` and `ToolCallResponse` for the failed call, with no
`InquiryRequest`.
Local-tool stdout is parsed by `serde_json::from_str::<Outcome>`
(`crates/jp_llm/src/tool.rs`), which falls back to `RawOutput` on parse errors.
The ingestion path MUST distinguish a `NeedsInput` carrying an invalid
`question.id` (which logs `error!` and returns a tool-level error) from stdout
that is not an `Outcome` (which falls back to `RawOutput`), so the fallback does
not swallow the invalid-inquiry diagnostic.

Because `question_id` and `attempt` cannot contain dots, the three-segment join
is injective and IDs are **unique by construction within the turn**, even when a
provider-generated `tool_call_id` contains dots: the last two dot-separated
tokens are always `question_id` and `attempt`, and everything before them is
`tool_call_id`.

`TurnState` tracks the counter in memory per `(tool_call_id, question_id)` key.
The counter increments when `InquiryRequest` is recorded, finalizing the ID
before the request is persisted, and resets at turn boundaries when a fresh
`TurnState` is created.
Nothing on disk encodes the counter beyond the IDs it produces.

**Readers MUST treat `InquiryId` as opaque.** Do not parse segments to extract
`tool_call_id`, `question_id`, or `attempt`.
Writers have these fields on `TurnState` and `ExecutingTool`; readers have them
on `InquiryRequest.question` and the surrounding `ToolCallRequest`.

Pre-082 streams use the two-segment form `<tool_call_id>.<question_id>`.
Two-segment IDs can collide within a turn when a tool re-emits the same
`question_id` after an invalid answer, because pre-082 `handle_tool_result`
recorded that case on the inquiry-backend path.
To read both formats without a migration step, read-side pairing accepts both
shapes and falls back to request/response order within the turn when IDs collide
(matching the strategy used for tool-call IDs).
New writes use the three-segment form and cannot collide.
Legacy streams also predate the no-dot rule on `question_id`, which is why the
order fallback remains on read.

`InquiryId` is the **stream-correlation** identifier for the persisted audit
trail and is distinct from two other identifiers that share parts of its shape:

- The in-memory `TurnState` cache key (`<tool_name>.<question_id>`, see
  [Splitting the turn-cache state](#splitting-the-turn-cache-state))
  deduplicates "remember for turn" answers across tool calls within the same
  turn.
- The tool-facing `ExecutingTool::accumulated_answers` map is keyed by the bare
  `question.id` with latest-answer-wins semantics.
  Tools do not see `tool_call_id` or `attempt`; if a tool re-asks the same
  `question.id`, the new answer overwrites the previous entry in this map, while
  the inquiry stream preserves every attempt.
  Tools that need to distinguish multiple instances of a question must use
  distinct `question.id` values.

### Source attribution

`InquirySource` is derived from **the JP code path that constructs the inquiry
event**, not from tool output or user config.

Built-in tools use a `BuiltinTool` trait hook:

```rust
pub trait BuiltinTool {
    // existing methods…

    /// The persisted `InquirySource` for questions emitted by this tool.
    ///
    /// Default: `InquirySource::Tool { name }`. Override for tools whose
    /// questions are semantically the assistant's, not the tool's
    /// (e.g. `ask_user`).
    fn inquiry_source(&self, name: &str) -> InquirySource {
        InquirySource::Tool { name: name.to_owned() }
    }
}
```

[RFD 083]'s `ask_user` overrides this method to return
`InquirySource::Assistant`.
RFD 082 ships no production overrides; tests exercise the `Assistant` path with
a synthetic built-in tool.

For MCP and local (shell-command-backed) tools, the wrapper always constructs
`InquirySource::Tool { name }`.
Neither tool type can override the persisted source: the MCP protocol exposes no
such field, JP's MCP integration does not accept one, and local tool commands
and arguments cannot alter provenance.

User config has no input into `InquirySource`.
The display-side "who's asking" label is configured separately through [RFD
083]'s `QuestionConfig.prompt_label` field:

| Concern                     | Source                                  |
| --------------------------- | --------------------------------------- |
| Persisted `InquirySource`   | `BuiltinTool::inquiry_source()` (or the |
|                             | wrapping code path for non-built-ins).  |
|                             | Not user-overridable.                   |
| Prompt "who's asking" label | `QuestionConfig.prompt_label`           |
|                             | (display-only). User-overridable.       |

#### Crossing the executor boundary

`BuiltinTool` lives in `jp_llm` and runs inside
`ToolDefinition::execute_builtin()`.
`ToolCoordinator` (in `jp_cli`) sits above the `Executor` trait and cannot
inspect the underlying tool's source kind directly.
To keep the coordinator free of tool-type branching, the `Executor` layer
resolves `InquirySource` before the coordinator receives a question.

`ExecutorResult::NeedsInput` gains a `source: InquirySource` field.
`ToolExecutor` (in `jp_cli`, which holds the `BuiltinExecutors` registry via
`TerminalExecutorSource`) populates `source` when converting
`ExecutionOutcome::NeedsInput` into `ExecutorResult`:

- `ToolSource::Builtin { .. }`: look up the `BuiltinTool` in the registry and
  call `inquiry_source(name)`.
- `ToolSource::Local { .. }` and `ToolSource::Mcp { .. }`: use
  `InquirySource::Tool { name }`.

The coordinator records the resolved value verbatim.
Source resolution lives in `ToolExecutor` alone, so adding a new tool source
kind requires no changes at recording sites.

Mock executors and `TestExecutorSource` populate `source` directly, letting test
fixtures exercise `Assistant` attribution without registering a real built-in
tool.

### Recording lifecycle

For every `Outcome::NeedsInput { question }` that passes `QuestionId` validation
(see [Inquiry ID format](#inquiry-id-format)):

1. Read `InquirySource` from `ExecutorResult::NeedsInput` (see [Crossing the
   executor boundary](#crossing-the-executor-boundary)).

2. Record `InquiryRequest { id, source, question }` **before any routing
   decision**, including cached-answer and static-answer short-circuits.
   The ID uses the three-segment format from [Inquiry ID
   format](#inquiry-id-format), and the persisted `InquiryQuestion` preserves
   the source `Question`'s `answer_type`, including `AnswerType::Secret`.

3. Check for an automatic answer (without showing a prompt):

   - **Cached answer** (`remembered_tool_answers`, from a previous `Y`/`N` in
     the same turn).
     Skipped for `AnswerType::Secret` questions, which are never cached (see
     [Secret questions](#secret-questions)).
     For other answer types, if a cached value exists, record
     `InquiryResponse::Answered { id, answer }` and resume tool execution.
   - **Static answer** (`QuestionConfig.answer`).
     If configured, pass the value to the tool and resume execution.
     Record `InquiryResponse::Answered { id, answer }` for non-`Secret` answer
     types, or `InquiryResponse::Redacted { id }` for `AnswerType::Secret`.

4. Otherwise, route the question to the interactive prompter or the inquiry
   backend.
   For `AnswerType::Secret` questions, the prompter disables input echo (see
   [Secret questions](#secret-questions) for routing restrictions).

5. On a successful answer:

   - Non-`Secret` answer type (from prompter or inquiry backend): record
     `InquiryResponse::Answered { id, answer }`.
   - `AnswerType::Secret` (from prompter only; the routing guard in [Secret
     questions](#secret-questions) blocks the inquiry backend): deliver the
     answer to the tool in memory and record `InquiryResponse::Redacted { id }`.

6. On a non-answer outcome, record `InquiryResponse::Cancelled { id, reason }`
   to close the persisted `InquiryRequest`/`InquiryResponse` pair.
   The `ToolCallResponse` returned to the LLM keeps its existing wording.
   The originating event determines `reason`:

   | Originating event                                                                          | `reason`                 |
   | ------------------------------------------------------------------------------------------ | ------------------------ |
   | `ExecutionEvent::PromptCancelled` (Esc, Ctrl-C, or EOF at the prompt)                      | `User`                   |
   | `ExecutionEvent::PromptCancelled` (prompter failure: I/O error, no TTY)                    | `BackendError`           |
   | `InquiryError::Cancelled` (cancellation token fired: user restart, tool cancel, hard quit) | `User`                   |
   | `InquiryError::Provider`                                                                   | `BackendError`           |
   | `InquiryError::MissingStructuredData`                                                      | `BackendError`           |
   | `InquiryError::AnswerExtraction { .. }`                                                    | `BackendError`           |
   | `InquiryError::Other(_)` (mock-backend catch-all)                                          | `BackendError`           |
   | `AnswerType::Secret` and no TTY available                                                  | `NoPromptBackend`        |
   | `AnswerType::Secret` and `target = "assistant"`                                            | `AssistantRoutingDenied` |

   Although `InquiryError::Cancelled` returns `Err` from the inquiry backend
   (`crates/jp_cli/src/cmd/query/tool/inquiry.rs` around
   `cancellation_token.cancelled()`), it represents a user-initiated
   cancellation: `interrupt/signals.rs` cancels the token on user actions
   (`InterruptAction::RestartTool`, `ToolCancelled`, hard quit).
   Mapping it to `User` rather than `BackendError` keeps the audit trail
   accurate.
   Similarly, `ExecutionEvent::PromptCancelled` maps `inquire` cancel and
   interrupt errors to `User`, and maps all other prompter errors (I/O, missing
   TTY, writer failure) to `BackendError`.

`Cancelled` records in the audit trail that the inquiry closed without an
answer; it does not encode retry policy.
The model-facing `ToolCallResponse` text remains the sole channel for retry
guidance to the model.
`Outcome::Error { transient }` in `jp_tool` is dropped at the executor boundary
(`crates/jp_llm/src/tool.rs` around `execute_builtin`); a future RFD that wants
persisted retryability should add a dedicated field or variant rather than
overloading `CancellationReason`.

### Secret questions

Some tool questions ask for values that should not be written to the
conversation stream, such as SSH passphrases, API keys, and one-time tokens.
Recording the question text while omitting the answer preserves the audit trail
without persisting the secret.

`jp_tool::AnswerType` gains a `Secret` variant for free-form text input whose
answer must not be persisted:

```rust
pub enum AnswerType {
    Boolean,
    Select { options: Vec<String> },
    Text,
    /// Free-form text input whose answer is not persisted on disk.
    /// Prompter input is not echoed; the persisted `InquiryResponse`
    /// is `Redacted { id }`, not `Answered`; and the turn-answer
    /// cache does not store or read answers for this question.
    Secret,
}
```

Encoding secrecy as an `AnswerType` variant rather than a `secret: bool` flag on
`Question` makes no-echo input and `Redacted` persistence type-level properties:
`Secret` is always free-form text, making ambiguous "secret boolean" or "secret
select" combinations unrepresentable.
`jp_conversation::event::inquiry::InquiryAnswerType` gains a matching `Secret`
variant, which serializes as `{"type": "secret"}` under the existing `tag =
"type"`, `rename_all = "snake_case"` serde attributes.

The answer type is a **tool-author declaration**, set by the code that
constructs `Question` (or by the JSON emitted on stdout for local tools).
User config cannot override it because secrecy is a property of *what* is asked,
not who answers or how:

- **Built-in tools** set `AnswerType::Secret` when constructing `Question`.
- **Local tools** return `"answer_type": {"type": "secret"}` in the
  `needs_input` JSON outcome on stdout, which `serde_json::from_str::<Outcome>`
  in `crates/jp_llm/src/tool.rs` deserializes directly.
- **MCP tools** cannot declare `Secret` because the MCP protocol and JP's MCP
  integration expose no corresponding field.
  A future RFD can add an MCP annotation if needed.

`Question.default` is a display hint (a pre-selected option or suggested value)
and propagates to `InquiryQuestion.default` verbatim regardless of answer type.
Tool authors MUST NOT put a sensitive literal in `default` on an
`AnswerType::Secret` question; JP does not redact it.

**Scope.** `AnswerType::Secret` has three effects:

1. **Prompter no-echo input.** `PromptBackend` gains a no-echo input method
   (such as `password`), implemented on `TerminalPromptBackend` via
   `inquire::Password` and on `MockPromptBackend` as a regular queued response.
   `ToolPrompter::prompt_question` dispatches to it when `question.answer_type
   == AnswerType::Secret`.
2. **`Redacted` persistence.** Every path that records
   `InquiryResponse::Answered` for non-`Secret` questions instead records
   `InquiryResponse::Redacted { id }` when `question.answer_type ==
   AnswerType::Secret`.
3. **Routing guard.** `AnswerType::Secret` questions cannot be routed to the
   inquiry backend.
   The coordinator enforces two fail-fast checks before routing:
   - No TTY available: fail the tool with a tool-level error and record
     `InquiryResponse::Cancelled { reason: NoPromptBackend }`.
   - Explicit `target = "assistant"`: fail the tool with a tool-level error and
     record `InquiryResponse::Cancelled { reason: AssistantRoutingDenied }`.
     Both checks match on `question.answer_type == Secret`, sharing the guard
     machinery that [RFD 083] reuses for `exclusive: true`.

**What `Secret` does and does not cover.** The guarantee is scoped to JP's
handling of the inquiry: JP disables prompt echo, records `Redacted { id }`
instead of the answer in `InquiryResponse`, and skips the `TurnState`
turn-answer cache.
It does **not** redact the value from other places where it might reach disk:

- **Tool output.** If a tool echoes the secret in its success or error text,
  `ToolCallResponse` persists that text verbatim.
- **Config values.** A secret set in `QuestionConfig.answer` lives in the user's
  config file and can be persisted in the conversation stream as a config delta,
  independent of the `Redacted` response.
- **`Question.default`** propagates verbatim and is not redacted.
- **Logs** and external tool behavior are out of scope.

Tracking a secret as a tainted value across `ToolCallResponse`, config deltas,
and logs belongs in a separate RFD.

**Static-answer shape is not validated.** `QuestionConfig.answer` accepts any
JSON value, and RFD 082 does not check that a static answer for a `Secret`
question is a string (matching the existing behavior for `Boolean`, `Select`,
and `Text`).
The configured value is passed to the tool in memory and the response is
recorded as `Redacted` regardless of shape, so a malformed static answer never
leaks a secret.
Answer-shape validation (`CancellationReason::InvalidStaticAnswer`) is
introduced in [RFD 083].

### `InquiryResponse` serialization

The widened `InquiryResponse` enum makes invalid states (both answered and
cancelled, or neither) unrepresentable and carries an explicit reason on
`Cancelled` so replay and debugging can distinguish non-answer outcomes:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum InquiryResponse {
    Answered { id: InquiryId, answer: Value },
    Cancelled { id: InquiryId, reason: CancellationReason },
    Redacted { id: InquiryId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancellationReason {
    /// The user explicitly cancelled (e.g. Ctrl-C at the prompt).
    User,
    /// The routing backend (prompter or inquiry backend) returned an
    /// error instead of an answer.
    BackendError,
    /// A question that requires a human answer (`AnswerType::Secret`,
    /// or [RFD 083]'s `exclusive: true`) could not be routed because
    /// no interactive terminal is available.
    NoPromptBackend,
    /// A question that requires a human answer was configured with
    /// `target = "assistant"` and refused to route to the inquiry
    /// backend.
    AssistantRoutingDenied,
    /// A reason produced by a JP version that knew a variant this build
    /// does not. The payload is the unparsed serde tag, preserved
    /// verbatim. Audit-trail-only — readers MUST NOT branch on the
    /// contents.
    Unknown(String),
}
```

`CancellationReason` uses custom `Serialize` and `Deserialize` implementations.
On disk it serializes as `"reason": "<tag>"`: `User` as `"user"`, `BackendError`
as `"backend_error"`, and `Unknown(s)` as `s` verbatim.
Deserialization maps any unrecognized tag to `Unknown(tag)` instead of failing,
so an older JP build can load streams written by a newer build without losing
the tag.

JP **never writes `Unknown(_)`** for its own events; `Unknown` is a read-only
fallback for unrecognized tags.
Future RFDs add sibling variants in the same module: [RFD 083] adds
`InvalidStaticAnswer` when `QuestionConfig.answer` does not match the question's
`answer_type` or `options`, and reuses `NoPromptBackend` and
`AssistantRoutingDenied` for `exclusive: true` routing guards.

`Redacted` carries only `id` (no `answer` or `reason` field), representing a
question whose answer was delivered to the tool in memory but omitted from disk
(see [Secret questions](#secret-questions)).

`Serialize` writes the internally-tagged form, matching `InquirySource` in the
same module:

```json
{
  "outcome": "answered",
  "id": "call_1.confirm.1",
  "answer": true
}
{
  "outcome": "cancelled",
  "id": "call_1.confirm.1",
  "reason": "user"
}
{
  "outcome": "cancelled",
  "id": "call_1.confirm.1",
  "reason": "backend_error"
}
{
  "outcome": "cancelled",
  "id": "call_1.passphrase.1",
  "reason": "no_prompt_backend"
}
{
  "outcome": "cancelled",
  "id": "call_1.passphrase.1",
  "reason": "assistant_routing_denied"
}
{
  "outcome": "cancelled",
  "id": "call_1.confirm.1",
  "reason": "some_future_variant"
}
{
  "outcome": "redacted",
  "id": "call_1.passphrase.1"
}
```

A custom `Deserialize` implementation for `InquiryResponse` accepts both the
tagged form and the pre-082 untagged form:

```json
{
  "id": "call_1.answer",
  "answer": true
}
```

Legacy events without an `outcome` field deserialize as
`InquiryResponse::Answered`.
An event missing `outcome`, `answer`, and a `Redacted`-shaped payload fails
deserialization, since pre-082 streams contain neither cancellations nor
redactions.
When `outcome == "cancelled"` is present without a `reason` field, `Deserialize`
defaults `reason` to `CancellationReason::User`, the conservative choice given
that `BackendError` and `Unknown` require JP itself to have written a reason.

Under this write-new, read-both scheme, existing conversation streams continue
to load without a migration step, and new writes use the tagged form.

### Reader behavior

Existing readers of the persisted stream handle the widened `InquiryResponse` as
follows:

- **Markdown export** (`crates/jp_cli/src/editor.rs`):
  - `Answered { answer }`: `Answer: <value>` (unchanged).
  - `Redacted { id }`: `Answer: <redacted>`.
  - `Cancelled { reason }`: `Cancelled (<reason>)`, where `<reason>` is the
    variant's serde tag (`user`, `backend_error`, `no_prompt_backend`,
    `assistant_routing_denied`, or the verbatim `tag` from `Unknown(tag)`).
    Derive this string from the variant's serde representation (for example via
    `CancellationReason::as_str`) so new `CancellationReason` variants render
    without changes to the exporter.
- **`jp conversation grep`** (`crates/jp_cli/src/cmd/conversation/grep.rs`):
  continues to ignore `InquiryResponse`, which carries no greppable text.
- **Turn renderer** (`crates/jp_cli/src/render/turn.rs`): continues to skip
  inquiry events.
  Formatting inquiry events in `jp conversation show` is out of scope (see
  [Non-Goals](#non-goals)).

All other readers that match on `InquiryResponse` MUST handle `Answered`,
`Cancelled`, and `Redacted`.

### Splitting the turn-cache state

`TurnState.persisted_inquiry_responses: IndexMap<InquiryId, InquiryResponse>`
serves two unrelated callers under one map:

- Tool-question answers, keyed `"<tool_name>.<question_id>"`, written and read
  by `handle_tool_result` and `handle_prompt_answer`.
- Tool-permission decisions, keyed `"<tool_name>.__permission__"`, written and
  read by `decide_permission` and `apply_permission_result`, gated by the
  permission prompt's `persist` flag.

Once `InquiryResponse` includes `Cancelled`, storing `InquiryResponse` values in
`TurnState` conflates audit-log events with reusable cached answers: a
`Cancelled` event is never a valid remembered answer for either caller, and
`persisted_inquiry_responses` is a misnomer once the values are no longer
`InquiryResponse` entries.

`TurnState` splits this map into two separate maps:

```rust
pub struct TurnState {
    /// Tool-question answers remembered for the duration of the turn,
    /// keyed `"<tool_name>.<question_id>"`.
    pub remembered_tool_answers: IndexMap<String, Value>,

    /// Tool-permission decisions remembered for the duration of the
    /// turn, keyed `"<tool_name>.__permission__"`. Gated by the
    /// permission prompt's `persist` flag. Values are
    /// `Value::String("y" | "n")` to match today's read sites.
    pub remembered_permission_decisions: IndexMap<String, Value>,

    // existing: request_count
}
```

The cache keys (`<tool_name>.<question_id>` and `<tool_name>.__permission__`)
are distinct from the stream-correlation `InquiryId` format (see [Inquiry ID
format](#inquiry-id-format)), whereas `TurnState` previously used `InquiryId`
for both.
The implementation may introduce dedicated newtypes per map, though RFD 082 does
not require them.

Each caller owns one map.
Permission reads check the prompt's `persist` flag as before, and tool-answer
reads preserve their existing behavior ([RFD 083] adds a persistence predicate
when it introduces `Question.persistence`).
Storing `Value` rather than `InquiryResponse` ensures a `Cancelled` or
`Redacted` response in the stream can never be read back as a remembered answer.

### Pre-seed removal

Before tool execution, the coordinator pre-seeds `accumulated_answers` from
`static_answers_for_all_questions`
(`crates/jp_cli/src/cmd/query/tool/coordinator.rs:805`).
Because the tool receives those answers on its first invocation, it never emits
`Outcome::NeedsInput`, leaving the coordinator with no question metadata to
record in an `InquiryRequest`.

RFD 082 removes this pre-seed.
Static answers instead flow through the `static_answer` lookup in
`handle_tool_result` (`coordinator.rs:1323`), which applies the configured
answer after `NeedsInput` fires.
Each configured static answer adds one extra local tool invocation: the first
call returns `NeedsInput`, and the coordinator immediately respawns the tool
with the answer injected, without showing a prompt or making an LLM round-trip.
In exchange, every question/answer exchange records an
`InquiryRequest`/`InquiryResponse` pair regardless of how the answer is
resolved.

**This changes the contract for user-supplied local tools.** MCP tools are
unaffected because `execute_mcp` never receives the `answers` map.
Built-in tools receive `answers` through `BuiltinTool::execute(&self, arguments,
answers)`, so removing the pre-seed changes what a built-in sees on its first
call; in-tree built-ins already use the two-call `NeedsInput → Success` pattern
and do not read `answers` on the first call, which Phase 3 verifies.
Local tools receive accumulated answers in their JSON context under `"tool": {
"answers": answers, ... }` (`crates/jp_llm/src/tool.rs` around line 719).
`QuestionConfig.answer` is documented as "the question will not be presented to
the target, but will always be answered with the given value"
(`crates/jp_config/src/conversation/tool.rs:1240-1241`), allowing a local tool
to expect the answer on its first invocation.
With pre-seeding removed, static answers arrive on the second invocation after
the tool emits `NeedsInput`.

Local tools that already follow the `NeedsInput → Success` pattern are
unaffected.
Local tools that inspect `tool.answers` on the first call without emitting
`NeedsInput` must switch to the two-call pattern.
Tools that perform side effects between argument validation and the first
`NeedsInput` will run those side effects once more per configured static answer.
The `QuestionConfig.answer` documentation is updated alongside this change.

### Coordinator plumbing

To record inquiry events on the prompter path, `handle_prompt_answer` and
`handle_prompt_cancelled` gain a `conv: &ConversationMut` parameter (matching
`handle_tool_result`).

The coordinator allocates the `InquiryId` once when recording `InquiryRequest`
(incrementing the turn-scoped counter) and threads it through the prompt-event
types so the answer and cancellation handlers can write the matching
`InquiryResponse` without reconstructing the ID:

```rust
enum PendingPrompt {
    Question {
        index: usize,
        question: Question,
        inquiry_id: InquiryId, // new
    },
    // …other variants unchanged
}

enum ExecutionEvent {
    PromptAnswer {
        index: usize,
        question_id: String,
        inquiry_id: InquiryId,        // new
        answer: Value,
        persist_level: jp_tool::PersistLevel,
    },
    PromptCancelled {
        index: usize,
        inquiry_id: InquiryId, // new
    },
    // …other variants unchanged
}
```

`PromptAnswer` and `PromptCancelled` both gain `inquiry_id`.
`PromptAnswer` retains `question_id` because
`ExecutingTool::accumulated_answers` is still keyed by `question.id`.
`PromptCancelled` needs only `inquiry_id` because the turn-scoped counter is
already incremented when the request is recorded.
Threading `inquiry_id` avoids reconstructing the ID from its segments in the
answer and cancellation handlers, preserving the opacity rule in [Inquiry ID
format](#inquiry-id-format).

Mapping inquiry errors to `CancellationReason` (see [Recording
lifecycle](#recording-lifecycle)) requires the typed `InquiryError` at the
recording site.
Because `spawn_inquiry` converts errors to strings (`.map_err(|e|
e.to_string())`) for `ExecutionEvent::InquiryResult.result: Result<Value,
String>`, the coordinator cannot distinguish error variants without parsing
error text.
Widening `InquiryResult.result` to `Result<Value, InquiryError>` (or mapping to
`CancellationReason` in `spawn_inquiry`) derives the reason from the error
variant while keeping the model-facing `ToolCallResponse` text unchanged.

## Drawbacks

- **Stream-size growth.** Recording prompter-answered questions adds two events
  (`InquiryRequest` and `InquiryResponse`) per tool-question round-trip where
  none were written before.
  Each event is small (the question text and a JSON answer value), and growth is
  bounded by the number of questions tools ask.
- **Static-answer contract change for user-supplied local tools.** Without
  pre-seeding, tools with a configured static answer execute twice (emit
  `NeedsInput`, then receive the injected answer) instead of once.
  MCP tools are unaffected because `execute_mcp` receives no `answers` map, and
  in-tree built-in tools do not read `answers` on their first call.
  Local tools that read `tool.answers` on the first invocation without emitting
  `NeedsInput` must switch to the two-call pattern, and tools with side effects
  before their first `NeedsInput` run those side effects one extra time per
  configured static answer (see [Pre-seed removal](#pre-seed-removal)).
- **Touches existing tool-question call sites.** Unified recording applies to
  every tool that emits `Outcome::NeedsInput`, not only `ask_user`, so existing
  callers need regression testing.
- **Widening `InquiryResponse` affects every stream reader.** Although custom
  `Deserialize` preserves backward compatibility with pre-082 events, readers
  that match on `InquiryResponse` must handle `Cancelled` and `Redacted`.
- **`InquirySource` plumbed through `ExecutorResult::NeedsInput`.** Adding
  `source` to `ExecutorResult::NeedsInput` touches every `Executor`
  implementation, including `MockExecutor` and test fixtures that construct
  `ExecutorResult` directly, in exchange for keeping source resolution in one
  place.
- **`AnswerType` gains `Secret` and an internally-tagged serde representation.**
  Both `jp_tool::AnswerType` and
  `jp_conversation::event::inquiry::InquiryAnswerType` add `Secret`, requiring a
  new match arm in the prompter, editor renderer, and other answer-type
  consumers.
  `jp_tool::AnswerType` also adds `#[serde(tag = "type", rename_all =
  "snake_case")]` to match `InquiryAnswerType` and the local-tool JSON examples.
  This **changes the local-tool stdout protocol** from externally tagged
  (`"Text"`, `{"Select": {"options": [...]}}`) to internally tagged (`{"type":
  "text"}`, `{"type": "select", "options": [...]}`), without a compatibility
  shim.
  The only in-tree consumer is `.config/jp/tools`, which serializes `Question`
  values through `jp_tool`; any raw JSON local-tool stdout fixtures in tests
  must be updated at implementation time.
  Persisted conversation streams are unaffected because they store
  `InquiryAnswerType` (already internally tagged via
  `tool_question_to_inquiry_question`), and pre-082 streams contain no `Secret`
  answers.
- **`PromptBackend` gains a no-echo input method.** Adding a method such as
  `password` requires implementations on `TerminalPromptBackend` (via
  `inquire::Password`), `MockPromptBackend`, and any other in-tree implementors.

## Alternatives

### Keep [RFD 005]'s prompter exception

Keep [RFD 005]'s exception for prompter-answered questions and record only the
`ToolCallRequest`/`ToolCallResponse` pair.
Rejected: question/answer exchanges have the same archival value whether the
user or the inquiry backend answered.
Omitting the prompter path bakes an implementation gap into the data model and
forces future features that need Q&A visibility to work around it.

### Record only when source is `Assistant`

Record inquiries for the inquiry backend (as in [RFD 005]) and when the source
is `Assistant` (for `ask_user`), while leaving user-prompted tool questions off
the stream.
Rejected: archival value does not depend on `InquirySource`.
This would preserve [RFD 005]'s three-path split and force future features that
need stream-level Q&A visibility to handle the same special case.

### Derive `InquirySource` from `QuestionConfig`

Earlier drafts of [RFD 083] proposed `QuestionConfig.attribution`, a
user-overridable enum that controlled both the prompt label and the persisted
`InquirySource`.
Rejected: a local config bundle could make any tool's questions persist as
assistant-sourced, undermining the audit trail that [RFD 005] establishes.
The `BuiltinTool` trait hook keeps `InquirySource` fixed at compile time with no
user override.

### Defer request/response uniqueness to a stream-entry-level ID

Keep the two-segment `InquiryRequest.id` (`<tool_call_id>.<question_id>`), rely
on a separate per-event UUID or ULID for stream-entry uniqueness, and pair
requests with responses by stream order.
Rejected: pair correlation and stream-entry uniqueness are separate contracts.
Pairing by stream order is fragile under manual edits, parallel tool calls, and
stream repair passes.
Encoding `attempt` in `InquiryId` (see [Inquiry ID format](#inquiry-id-format))
keeps pair correlation self-contained without depending on another RFD.

### Generate `InquiryRequest.id` from a UUID or ULID

Generate opaque UUID or ULID values instead of
`<tool_call_id>.<question_id>.<attempt>`.
Rejected: structured IDs are readable in raw stream files, manual edits, and
`grep` output, while a turn-scoped attempt counter provides uniqueness within
each turn.

## Non-Goals

- **Rendering inquiry events in `jp conversation show`.** CLI display formatting
  for inquiry events remains deferred, matching [RFD 005].
- **Inquiry events for permission and result-delivery prompts.** This RFD covers
  tool questions (`Outcome::NeedsInput`); `RunTool` and `DeliverToolResult`
  prompts are not inquiry events and remain out of scope.
- **Stream replay and event-driven UI.** Sub-agent reasoning trails, replay
  tooling, and conversation viewers benefit from complete inquiry recording, but
  are separate features.
- **Migration tooling for legacy streams.** Custom `Deserialize` reads legacy
  events in place while new writes use the tagged format, so no standalone
  migration step is needed.

## Risks and Open Questions

- **Substantive amendment to [RFD 005].** RFD 082 updates the "Recording Inquiry
  Events" section of [RFD 005] to remove the prompter exception, update the
  routing table, and document cancellation recording.
  This amendment lands with the RFD 082 implementation.
- **[RFD 083] is a downstream consumer, not a dependency.** RFD 082 ships
  against existing types without waiting on [RFD 083].
  [RFD 083] depends on RFD 082: it adds `exclusive: bool` and `persistence:
  AnswerReusePolicy` to `jp_tool::Question` (with cache-persistence gating and
  `exclusive` routing checks in the coordinator), adds `context`, `exclusive`,
  and `persistence` to `InquiryQuestion`, overrides
  `BuiltinTool::inquiry_source()` to return `InquirySource::Assistant` for
  `ask_user`, and adds `CancellationReason::InvalidStaticAnswer` for static
  answers that do not match the question's `answer_type` or `options`.
  RFD 082 ships `CancellationReason` with `User`, `BackendError`,
  `NoPromptBackend`, `AssistantRoutingDenied`, and `Unknown(String)`; [RFD 083]
  reuses `NoPromptBackend` and `AssistantRoutingDenied` for its `exclusive:
  true` routing guards.
- **Sensitive data exposure.** `AnswerType::Secret` covers passwords,
  passphrases, and API keys when tool authors declare a question as `Secret`: JP
  records `Redacted` instead of `Answered`, disables prompt echo, and refuses
  routing to the inquiry backend (on missing TTY or `target = "assistant"`).
  As noted in [Secret questions](#secret-questions), JP does not redact secrets
  from `ToolCallResponse` output, config files or config deltas
  (`QuestionConfig.answer`), `Question.default`, or logs, nor does it redact
  sensitive values entered into non-`Secret` questions.
  Richer redaction policies such as tainted-value tracking belong in a future
  RFD.
- **Stream-size growth for prompt-heavy tools.** Tools that prompt frequently
  write two additional small events per question.

## Implementation Plan

### Phase 1: `BuiltinTool::inquiry_source()` hook and executor-boundary plumbing

Add `BuiltinTool::inquiry_source(&self, name: &str) -> InquirySource` with the
default `InquirySource::Tool { name }` implementation.

Add `source: InquirySource` to `ExecutorResult::NeedsInput`.
Update `ToolExecutor::execute` to populate `source` from
`BuiltinTool::inquiry_source()` for `ToolSource::Builtin { .. }` and from
`InquirySource::Tool { name }` for local and MCP tools.
Update `MockExecutor` and `TestExecutorSource` fixtures to set `source`, and
update the coordinator to read `ExecutorResult::NeedsInput.source` instead of
constructing `InquirySource::tool(name)`.

This phase introduces no behavior change and can merge independently.

### Phase 2: Widen `InquiryResponse` and split `TurnState`

Add `InquiryResponse::Cancelled` with `CancellationReason::{User, BackendError,
NoPromptBackend, AssistantRoutingDenied, Unknown(String)}` and
`InquiryResponse::Redacted`.
Implement custom `Serialize` and `Deserialize` for `CancellationReason` so
unrecognized tags round-trip as `Unknown(s)` verbatim.
Implement custom `Deserialize` for `InquiryResponse` that accepts the legacy
untagged form as `Answered` and maps a missing (or non-string) `reason` on a
tagged `Cancelled` event to `Unknown` with the sentinel tag `unspecified` (or
`User` per the deserialization rules in [`InquiryResponse`
serialization](#inquiryresponse-serialization)).

Split `TurnState.persisted_inquiry_responses` into `remembered_tool_answers:
IndexMap<String, Value>` (used by `handle_tool_result` and
`handle_prompt_answer`) and `remembered_permission_decisions: IndexMap<String,
Value>` (used by `decide_permission` and `apply_permission_result`).
Use `String` keys rather than `InquiryId`, since the cache-key formats
(`<tool_name>.<question_id>` and `<tool_name>.__permission__`) differ from the
stream-correlation `InquiryId` format (see [Splitting the turn-cache
state](#splitting-the-turn-cache-state)).

Tests:

- **Backward-compat round-trip.** Deserialize pre-082 answered-response JSON and
  assert it produces `Answered`.
- **Tagged serialization.** Serialize `Answered`, `Cancelled { reason: User }`,
  `Cancelled { reason: BackendError }`, and `Redacted`, and assert each produces
  its expected tagged JSON shape.
- **Missing `reason` defaults to `User`.** Deserialize a tagged `cancelled`
  event without a `reason` field and assert it produces `Cancelled { reason:
  User }`.
- **Unknown `reason` round-trips through `Unknown(String)`.** Deserialize a
  cancelled event with `"reason": "some_future_variant"`, assert it produces
  `Cancelled { reason: Unknown("some_future_variant".into()) }`, and
  re-serialize to verify the JSON is identical.
- **`Redacted` carries no answer.** Serialize `Redacted` and assert the JSON has
  no `answer` field; deserialize `Redacted` and assert no answer is
  materialized.
- **Invalid shape.** Assert that deserializing an event with no `outcome`, no
  `answer`, and no `Redacted` payload fails.
- **Turn-cache split.** Round-trip a permission decision through
  `remembered_permission_decisions` and a tool answer through
  `remembered_tool_answers`; assert neither caller sees the other map's entries
  and that `Cancelled` and `Redacted` responses are never stored in either map.

Depends on Phase 1.

### Phase 3: Unified recording lifecycle

Recording-lifecycle plumbing:

- Audit every `BuiltinTool` implementation for first-call reads of `answers`,
  then remove the pre-execution seeding of `accumulated_answers` from
  `static_answers_for_all_questions`.
  In-tree built-ins (such as `DescribeTools`) ignore `answers` on the first
  call, and new built-ins (including [RFD 083]'s `ask_user`) must follow the
  two-call `NeedsInput → Success` pattern.
- Pass `&ConversationMut` to `handle_prompt_answer` and
  `handle_prompt_cancelled`.
- Allocate `InquiryId` when recording `InquiryRequest` and thread it through
  `PendingPrompt::Question`, `ExecutionEvent::PromptAnswer`, and
  `ExecutionEvent::PromptCancelled` (see [Coordinator
  plumbing](#coordinator-plumbing)).
- Record `InquiryRequest` before any routing decision, including cached-answer
  and static-answer short-circuits.
- Record `InquiryResponse::Answered` on cached, static, prompter, and
  inquiry-backend success paths for non-`Secret` questions.
- Record `InquiryResponse::Cancelled` with the mapped reason from [Recording
  lifecycle](#recording-lifecycle) (`User` for user cancellations including
  `InquiryError::Cancelled`, `BackendError` for backend failures, and
  `NoPromptBackend` or `AssistantRoutingDenied` for secret routing guards).
- Widen `ExecutionEvent::InquiryResult.result` from `Result<Value, String>` to
  carry `InquiryError` (or a `CancellationReason` mapped in `spawn_inquiry`) so
  the cancellation reason is derived from the error variant rather than parsed
  from error text (see [Coordinator plumbing](#coordinator-plumbing)).

Inquiry ID format:

- Add a per-`(tool_call_id, question_id)` attempt counter to `TurnState`,
  incremented when `InquiryRequest` is recorded (starting at `1`) and reset at
  turn boundaries when a fresh `TurnState` is created.
- Add the `QuestionId` newtype in `jp_tool` (wrapping `Question.id`) that
  rejects `.` in `FromStr` and `TryFrom`, and route local-tool stdout and
  built-in question construction through it.
  When a `NeedsInput` has an invalid `question.id`, log `error!`, fail the tool
  with a tool-level error response (rendered via `ErrorStyleConfig`), and drop
  the outcome before constructing any inquiry event.
  Ensure `serde_json::from_str::<Outcome>` distinguishes an invalid
  `question.id` from non-`Outcome` stdout so the `RawOutput` fallback does not
  swallow the error (see [Inquiry ID format](#inquiry-id-format)).
- Format `InquiryRequest.id` and its matching `InquiryResponse.id` as
  `<tool_call_id>.<question_id>.<attempt>`.
- Update readers that pair events by `InquiryId` to accept both two-segment and
  three-segment IDs, falling back to request/response order on duplicate legacy
  IDs.
- Scope orphan repair in `ConversationStream::sanitize()` to individual turns.
  `remove_orphaned_inquiry_responses` and `remove_orphaned_inquiry_requests`
  (`crates/jp_conversation/src/stream.rs`) previously built flat ID sets across
  the entire stream, which could match pairs across turns when `tool_call_id`
  values are reused.
  Keep the removal behavior (do not synthesize `Cancelled` events for orphans
  left by crashes, truncation, or manual edits) while restricting each ID set to
  a single turn, and apply the same turn scoping to any other full-stream
  `InquiryId` index.
  If [RFD 023] lands, incomplete-tail extraction runs before `sanitize()`, so
  per-turn orphan removal runs only on complete turns while unpaired events in
  an incomplete turn move into `IncompleteTurn`.

Secret-question handling:

- Add `Secret` to `jp_tool::AnswerType` and annotate the enum with `#[serde(tag
  = "type", rename_all = "snake_case")]` so its JSON representation (`{"type":
  "boolean"}`, `{"type": "select", "options": [...]}`, `{"type": "text"}`,
  `{"type": "secret"}`) matches `InquiryAnswerType`.
  Add `Question::secret(id, text) -> Result<Self, InvalidQuestionId>` alongside
  `Question::text`, `Question::boolean`, and `Question::select`, all validating
  `id` via `QuestionId`.
- Add `Secret` to `jp_conversation::event::inquiry::InquiryAnswerType`
  (serialized as `{"type": "secret"}`) and map it in
  `tool_question_to_inquiry_question`.
- Add a no-echo input method (such as `password`) to `PromptBackend`,
  implemented on `TerminalPromptBackend` via `inquire::Password` and on
  `MockPromptBackend` as a queued response, and dispatch to it in
  `ToolPrompter::prompt_question` when `question.answer_type ==
  AnswerType::Secret`.
- Skip `remembered_tool_answers` reads and writes for `Secret` questions.
  Record `InquiryResponse::Redacted { id }` instead of `Answered` on both the
  static-answer and prompter paths for `Secret` questions.
- Enforce the routing guards before routing a `Secret` question:
  - **No TTY available**: return a tool-level error response and record
    `InquiryResponse::Cancelled { reason: NoPromptBackend }`.
  - **Explicit `target = "assistant"`**: return a tool-level error response and
    record `InquiryResponse::Cancelled { reason: AssistantRoutingDenied }`.

Tests:

- **Attempt counter.** Emitting the same `question_id` twice within a single
  tool call produces `.1` and `.2` `InquiryRequest.id` values and two paired
  `InquiryResponse` events.
- **Cross-cycle uniqueness within a turn.** Reusing `tool_call_id` across cycles
  in the same turn (Gemini-style) with the same `question_id` produces `.1` and
  `.2` rather than resetting the counter.
- **Per-turn reset.** Two separate turns both start the counter at `1` for the
  same `(tool_call_id, question_id)` key.
- **Legacy ID pairing.** A pre-082 stream with two-segment IDs pairs requests
  and responses, falling back to stream order when two legacy events share an
  ID.
- **Secret prompter answer.** Answering a `Question { answer_type:
  AnswerType::Secret, .. }` at the prompter delivers the answer to the tool in
  memory and records `InquiryResponse::Redacted` with no answer value on disk.
- **Secret static answer.** A `Secret` question with a configured
  `QuestionConfig.answer` delivers the answer to the tool in memory and records
  `InquiryResponse::Redacted`.
- **Local-tool secret.** A local tool emitting `{"type": "needs_input",
  "question": {"answer_type": {"type": "secret"}, ...}}` on stdout receives the
  answer in `tool.answers` on its next invocation and records
  `InquiryResponse::Redacted` with no answer on disk.
- **Cache bypass.** A `Secret` question is neither written to nor read from
  `remembered_tool_answers`.
- **Non-secret regression.** `Boolean`, `Select`, and `Text` questions record
  `Answered` and use the turn cache as before.
- **Inquiry ID round-trip via prompt events.** An `InquiryId` allocated when
  recording `InquiryRequest` passes through `ExecutionEvent::PromptAnswer` so
  `handle_prompt_answer` writes the same ID on the response.
- **Cancellation reason mapping.** `InquiryError::Cancelled` maps to `Cancelled
  { reason: User }`; `InquiryError::Provider`, `MissingStructuredData`,
  `AnswerExtraction`, and `Other` map to `Cancelled { reason: BackendError }`.
- **Cross-turn ID collision under sanitize.** When an orphaned `InquiryRequest`
  in turn 1 shares its ID with a valid pair in turn 2,
  `ConversationStream::sanitize()` removes the turn-1 orphan instead of matching
  it against turn 2.
- **Legacy duplicate IDs within a turn under sanitize.** In a turn with two
  `InquiryRequest` events sharing a legacy two-segment ID and one matching
  response, `sanitize()` keeps the first pair by order and removes the remaining
  orphan.
- **Secret routing guard, no TTY.** A `Secret` question without a TTY fails the
  tool with a tool-level error response and records `Cancelled { reason:
  NoPromptBackend }`.
- **Secret routing guard, assistant target.** A `Secret` question with `target =
  "assistant"` fails the tool with a tool-level error response and records
  `Cancelled { reason: AssistantRoutingDenied }`.

A synthetic test tool overrides `BuiltinTool::inquiry_source()` to test
`Assistant` attribution; the production consumer (`ask_user`) lands in [RFD
083].

Depends on Phases 1 and 2.

### Phase 4: Amend [RFD 005]

Update the "Recording Inquiry Events" section of [RFD 005] to remove the
prompter exception, update the routing table, and document cancellation
recording.
Lands alongside Phase 3 in the same PR.

## References

- [RFD 005]: first-class inquiry events; amended by this RFD to record
  prompter-answered questions and add `Cancelled`.
- [RFD 023]: resumable conversation turns; consumes RFD 082.
  RFD 023 relies on persisted user-targeted `InquiryRequest`/`InquiryResponse`
  pairs to detect and resume incomplete turns blocked on inquiries, which RFD
  005's prompter exception left unrecorded.
  If RFD 023 is confirmed to depend on RFD 082, `just rfd-require` records the
  link on both sides (`Required by` on 082 and `Requires` on 023) at promotion
  time.
- [RFD 028]: the structured inquiry system that this recording layer builds on.
- [RFD 034]: defines `QuestionTarget`.
- [RFD 083]: introduces the `ask_user` tool (`Requires` RFD 082); overrides
  `BuiltinTool::inquiry_source()` to return `InquirySource::Assistant`, adds
  `context`, `exclusive`, and `persistence` to `InquiryQuestion`, adds
  `CancellationReason::InvalidStaticAnswer`, and reuses `NoPromptBackend` and
  `AssistantRoutingDenied` for `exclusive: true` routing guards.
- [RFD 049]: defines the `exclusive` flag and detached-policy cascade consumed
  by [RFD 083].

[RFD 005]: 005-first-class-inquiry-events.md
[RFD 005-2]: ./005-first-class-inquiry-events.md
[RFD 023]: 023-resumable-conversation-turns.md
[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 034]: 034-inquiry-specific-assistant-configuration.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 083]: 083-built-in-ask_user-tool-for-assistant-initiated-inquiries.md
