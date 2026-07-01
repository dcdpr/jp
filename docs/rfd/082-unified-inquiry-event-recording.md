# RFD 082: Unified inquiry event recording

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-12
- **Extends**: [RFD 005][RFD 005-2]
- **Required by**: [RFD 083]

## Summary

Record `InquiryRequest`/`InquiryResponse` events in the persisted conversation
stream for **every** `Outcome::NeedsInput` round-trip, regardless of routing
path.
Today [RFD 005] records only inquiry-backend questions; prompter-answered
questions, cached answers, and static answers leave no trace.
This RFD closes the gap.
It also adds an `InquiryResponse::Cancelled` variant so user cancellation and
routing-backend errors appear in the stream, an `InquiryResponse::Redacted`
variant so secret answers typed at the prompter never land on disk, and derives
the persisted `InquirySource` from non-overridable tool metadata rather than
from user-configurable fields.
Downstream RFDs may extend the `Cancelled` enum with additional reasons for the
routing paths they introduce.

## Motivation

[RFD 005] explicitly excludes prompter-answered questions: *"Questions answered
via interactive user prompts ... are not recorded as inquiry events."* That
carve-out was implementation-driven (the prompter path didn't carry a write
handle to the conversation stream), not principled.
The archival value of a question/answer exchange is the same regardless of
routing path.
Leaving the prompter path off the stream freezes an implementation gap into the
data model and forces every future feature that wants Q\&A visibility — replay,
debugging, sub-agent reasoning trails, conversation viewers — to re-derive the
workaround.

The `InquiryResponse` enum currently encodes only "answered."
Non-answer outcomes — user Ctrl-C and routing-backend errors — have no on-disk
representation, so the persisted stream is incomplete for any tool question that
didn't complete normally.
The inquiry-backend-failure case in particular leaves an `InquiryRequest`
without a matching response today, which 082 cannot ship without addressing once
it starts recording requests for every routing path.

`InquirySource` is set today from the tool name at the recording site:

```rust
InquirySource::tool(tool_name.clone())
```

This hard-codes provenance to the tool name.
Future tools whose questions are semantically the assistant's, not the tool's —
[RFD 083]'s `ask_user` is the motivating consumer — need to record those
inquiries as `Assistant`-sourced.
Doing this through a user-configurable field would make persisted provenance
overridable, which defeats the audit purpose: a local config bundle could
relabel any tool's question as assistant-sourced in the persisted record.
Doing it as a static tool-author declaration keeps the value compile-time-set
without name-branching in coordinator code. 082 adds the hook; consumers like
[RFD 083] use it.

## Design

### What changes for stream readers

Every tool-question round-trip produces an `InquiryRequest`/`InquiryResponse`
pair in the persisted stream, sitting between the `ToolCallRequest` and
`ToolCallResponse` for the tool.
Today the same shape already appears for inquiry-backend-resolved questions.
After this RFD, it appears uniformly for:

- Questions resolved by the interactive prompter (user typed an answer).
- Questions resolved by a cached "remember for turn" answer from earlier in the
  same turn.
- Questions resolved by a `QuestionConfig.answer` static value from user config.
- Questions whose answer type is `AnswerType::Secret` — the request is
  recorded; the response is recorded as `Redacted { id }` without the answer
  payload (see [Secret questions](#secret-questions)).
- Questions cancelled by the user (Ctrl-C at the prompt).
- Questions that failed because the chosen routing backend (the prompter or the
  inquiry backend) returned an error.

The matched pair carries the same `id`.
The ID is unique per request/response pair within the turn that contains it; see
[Inquiry ID format](#inquiry-id-format) below for the shape, the per-attempt
counter, and the turn-local scope of uniqueness.

### Inquiry ID format

`InquiryRequest.id` is the correlation key for the matching `InquiryResponse`
and MUST be unique **within the turn** that contains it.
IDs are NOT required to be unique across turns — `TurnMut` permits
`tool_call_id` reuse across turns (and across cycles within a turn — see
`same_tool_call_id_across_turns_is_allowed` and
`same_tool_call_id_reused_within_turn_across_cycles` in
`crates/jp_conversation/src/stream/turn_mut_tests.rs`), and the inquiry ID
inherits that across turns but not within them.
The format is:

```
<tool_call_id>.<question_id>.<attempt>
```

where `attempt` is a 1-indexed counter scoped to the `(tool_call_id,
question_id)` pair within the turn.
The first time a given `(tool_call_id, question_id)` is recorded in a turn,
attempt is `1`; the next recording for the same key (the LLM gave an invalid
answer and the tool re-asks; or a provider like Google Gemini reused the same
`tool_call_id` in a later cycle and the same question came up again) gets
attempt `2`; and so on.
IDs are unique by construction within the turn.

The counter is tracked per-turn (in `TurnState`) keyed by `(tool_call_id,
question_id)`.
It increments at the `InquiryRequest`-recording site, so the ID is finalized
before the request lands on disk.
The counter resets only at turn boundaries — a fresh `TurnState` is built per
turn.
The counter is in-memory only; nothing on disk encodes it beyond the IDs it
produces.

**Readers MUST treat `InquiryId` as opaque.** The segment shape is a writer-side
construction.
Do not parse segments to extract `tool_call_id`, `question_id`, or `attempt`.
The writer side has these fields available on `TurnState` and `ExecutingTool`;
the reader side has them on `InquiryRequest.question` and on the surrounding
`ToolCallRequest`.
No reader needs to recover them from the ID.

Pre-082 streams use the legacy two-segment form
(`<tool_call_id>.<question_id>`).
Two-segment IDs can collide within a turn if a tool re-emitted the same
`question_id` after an invalid answer — pre-082 `handle_tool_result` did record
this case via the inquiry backend path, so legacy streams may contain duplicate
IDs.
To read both formats without a migration step, the pairing logic on read accepts
both shapes and falls back to request/response order within the turn when IDs
collide (the same strategy used for tool-call IDs today).
Only new writes use the three-segment form, and new writes do not collide
because the counter is turn-scoped.

This ID is the **stream-correlation** identifier for the persisted audit trail.
It is distinct from two other identifiers that share some of the same parts:

- The in-memory `TurnState` cache key (`<tool_name>.<question_id>` — see
  [Splitting the turn-cache state] (\#splitting-the-turn-cache-state)) dedupes
  "remember for turn" answers across tool calls within the same turn.
- The tool-facing `ExecutingTool::accumulated_answers` map remains keyed by the
  bare `question.id`, with latest-answer-wins semantics.
  Tools have no access to `tool_call_id` or `attempt`; if a tool re-asks the
  same `question.id`, the new answer overwrites the previous one in this map.
  Tools that need to distinguish multiple instances of a logical question use
  distinct `question.id`s.
  The inquiry stream preserves every attempt; this map preserves only the most
  recent answer per question.

### Source attribution

`InquirySource` is derived from **which code path constructs the inquiry event
in JP**, not from the tool's content and not from user config.

For built-in tools, a new `BuiltinTool` trait hook:

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

[RFD 083]'s `ask_user` will override this to return `InquirySource::Assistant`.
082 itself ships no production overrides; tests exercise the assistant-source
path with a synthetic built-in.

For MCP tools and local tools, the wrapping code path always constructs
`InquirySource::Tool { name }`.
There is no API for these tool types to influence the persisted source — an MCP
server cannot claim to be the assistant, because the MCP protocol exposes no
such field and JP's MCP integration does not honor one.
Same for local (shell-command-backed) tools: their commands and arguments cannot
affect provenance.

User config has no input into `InquirySource`.
The display-side "who's asking" label is a separate concern handled by [RFD
083]'s `QuestionConfig.prompt_label` field.
Two independent properties:

| Concern                     | Source                                  |
| --------------------------- | --------------------------------------- |
| Persisted `InquirySource`   | `BuiltinTool::inquiry_source()` (or the |
|                             | wrapping code path for non-built-ins).  |
|                             | Not user-overridable.                   |
| Prompt "who's asking" label | `QuestionConfig.prompt_label`           |
|                             | (display-only). User-overridable.       |

#### Crossing the executor boundary

The `BuiltinTool` trait lives in `jp_llm` and is invoked deep inside
`ToolDefinition::execute_builtin()`.
The `ToolCoordinator` (in `jp_cli`) sits above the `Executor` trait and has no
direct view of the underlying tool's source kind.
To keep the coordinator free of tool-type branching, the `Executor` layer
resolves `InquirySource` before the coordinator ever sees a question.

`ExecutorResult::NeedsInput` is extended with a `source: InquirySource` field.
`ToolExecutor` (in `jp_cli`, which already holds the `BuiltinExecutors` registry
via `TerminalExecutorSource`) populates it when it converts
`ExecutionOutcome::NeedsInput` into `ExecutorResult`:

- For `ToolSource::Builtin { .. }`: look up the `BuiltinTool` in the registry
  and call `inquiry_source(name)`.
- For `ToolSource::Local { .. }` and `ToolSource::Mcp { .. }`: default to
  `InquirySource::Tool { name }`.

The coordinator records the already-resolved value verbatim.
There is no path from the coordinator to a per-tool source decision; the
resolution rule lives in exactly one place, and adding a new source kind is a
change to `ToolExecutor`, not to every recording site.

Mock executors and `TestExecutorSource` populate the field directly, so test
fixtures can exercise the `Assistant`-attribution path without wiring a real
built-in.

### Recording lifecycle

For every `Outcome::NeedsInput { question }`:

1. The coordinator reads the `InquirySource` carried on
   `ExecutorResult::NeedsInput`.
   Resolution happened at the executor boundary (see [Crossing the executor
   boundary] (\#crossing-the-executor-boundary)); the coordinator does not
   branch on tool type.

2. The coordinator records `InquiryRequest { id, source, question }` **before
   any routing decision** — including the cached-answer and static-answer
   short-circuits below.
   The recording uses the format defined in [Inquiry ID
   format](#inquiry-id-format), and the persisted `InquiryQuestion` carries the
   same `answer_type` as the source `Question` — including `AnswerType::Secret`
   when applicable.

3. The coordinator checks for an automatic answer:

   - **Cached answer** (`remembered_tool_answers` — a previous `Y`/`N` in this
     turn).
     Skipped for `AnswerType::Secret` questions; secret answers do not enter the
     cache (see [Secret questions] (\#secret-questions)).
     For non-`Secret` answer types: if found, record `InquiryResponse::Answered
     { id, answer }` with the cached value and resume tool execution.
     No prompt is shown.
   - **Static answer** (configured via `QuestionConfig.answer`).
     If present, apply the configured value and resume tool execution.
     For non-`Secret` answer types, record `InquiryResponse::Answered { id,
     answer }`; for `AnswerType::Secret`, record `InquiryResponse::Redacted { id
     }` even though the configured value is what the tool received.
     No prompt is shown.

4. Otherwise, route the question (interactive prompter or inquiry backend).
   For `AnswerType::Secret` questions on the prompter path, the prompter uses a
   no-echo input mode (see [Secret questions] (\#secret-questions) for the
   routing-scope caveat).

5. On a successful answer:

   - Non-`Secret` answer type (from prompter or inquiry backend): record
     `InquiryResponse::Answered { id, answer }`.
   - `AnswerType::Secret` (from prompter only — the inquiry backend is
     unreachable per the routing guard in [Secret questions]
     (\#secret-questions)): record `InquiryResponse::Redacted { id }`.
     The answer is delivered to the tool in-memory; only the persisted shape is
     redacted.

6. On a non-answer outcome, record `InquiryResponse::Cancelled { id, reason }`.
   The `ToolCallResponse` returned to the LLM is unchanged (existing wording);
   this only closes the `InquiryRequest`/`InquiryResponse` pairing on the
   persisted side.
   Without this, every recorded `InquiryRequest` whose backend errored or whose
   user cancelled would land unpaired on disk.
   The `reason` is determined by the originating event:

   | Originating event | `reason` | |
   ------------------------------------------------------ |
   ------------------------ | | `ExecutionEvent::PromptCancelled` (user Ctrl-C
   or EOF) | `User` | | `InquiryError::Cancelled` (cancellation token fired — |
   `User` | | user restart, tool cancel, hard quit) | | |
   `InquiryError::Provider` | `BackendError` | |
   `InquiryError::MissingStructuredData` | `BackendError` | |
   `InquiryError::AnswerExtraction { .. }` | `BackendError` | |
   `InquiryError::Other(_)` (mock-backend catch-all) | `BackendError` | |
   `AnswerType::Secret` and no TTY available | `NoPromptBackend` | |
   `AnswerType::Secret` and `target = "assistant"` | `AssistantRoutingDenied`
   |\` |

   `InquiryError::Cancelled` returns `Err` from the inquiry backend today (see
   `crates/jp_cli/src/cmd/query/tool/inquiry.rs` around
   `cancellation_token.cancelled()`), but it is semantically a user-initiated
   cancellation — the token is cancelled by `interrupt/signals.rs` in response
   to user actions (`InterruptAction::RestartTool`, `ToolCancelled`, hard quit).
   Mapping it to `User` (not `BackendError`) keeps the audit trail honest.
   Prompter I/O errors (e.g. EOF on stdin mid-prompt) are indistinguishable from
   Ctrl-C at the coordinator's vantage today and follow the same `User` path; if
   a future change distinguishes them, the right landing is a new
   `CancellationReason` variant rather than re-using `BackendError`.

`Cancelled` records that the inquiry was closed without an answer — for the
audit trail only.
It does not encode retry policy.
The model-facing `ToolCallResponse` text (existing wording, unchanged by 082) is
the sole channel for "may retry" / "do not retry" guidance to the model.
`Outcome::Error { transient }` exists in `jp_tool` and is dropped at the
executor boundary today (`crates/jp_llm/src/tool.rs` around `execute_builtin`);
if a future RFD wants persisted retryability, it adds a separate field or
variant rather than overloading `CancellationReason`.

### Secret questions

Some tool questions ask for values that must not be persisted on disk — SSH
passphrases, API keys, one-time tokens.
Recording the question text without the answer keeps the audit trail honest
without leaking the secret.

`jp_tool::AnswerType` gains a new variant, `Secret`, for free-form text input
whose answer must not be persisted:

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

Encoding secret-ness as an answer type, rather than as a `secret: bool` flag on
`Question`, makes the no-echo + `Redacted` behavior a type-level property:
`Secret` is by construction free-form text with no echo and no persistence, so
the ambiguous "secret boolean" or "secret select" shapes are unrepresentable.
`jp_conversation::event::inquiry::InquiryAnswerType` gains a matching `Secret`
variant; the existing serde tagging (`tag = "type"`, `rename_all =
"snake_case"`) serializes it as `{"type": "secret"}`.

The answer type is a **tool-author declaration**: it is set by the code that
constructs the `Question` (or, for local tools, by the JSON emitted on stdout).
User config has no input — secret-ness is a property of *what* is being asked,
not of who is answering or how.
Concretely:

- **Built-in tools** declare `AnswerType::Secret` in the `Question` literal they
  construct.
- **Local tools** declare it by returning `"answer_type": {"type": "secret"}` in
  the `needs_input` JSON outcome on stdout.
  The existing `serde_json::from_str::<Outcome>` path in
  `crates/jp_llm/src/tool.rs` deserializes the new variant with no further
  plumbing.
- **MCP tools** cannot declare it today.
  The MCP protocol exposes no field that maps to `AnswerType::Secret`, and JP's
  MCP integration does not surface one.
  A future RFD may add an MCP-side annotation if needed.

`Question.default` is a tool-author-set display hint (a pre-selected option or
suggested value).
It is propagated to `InquiryQuestion.default` verbatim regardless of the answer
type.
Tool authors MUST NOT set `default` to a sensitive literal on an
`AnswerType::Secret` question; JP does not redact it.

**Scope.** 082 gives the `Secret` variant three effects:

1. **Prompter no-echo input.** `PromptBackend` gains a no-echo input method
   (e.g. `password`), implemented on `TerminalPromptBackend` via
   `inquire::Password` and on `MockPromptBackend` as a regular queued response.
   `ToolPrompter::prompt_question` dispatches to it when `question.answer_type
   == AnswerType::Secret`.
2. **`Redacted` persistence.** Every code path that would record
   `InquiryResponse::Answered` for a non-`Secret` answer type records
   `InquiryResponse::Redacted { id }` when the question's answer type is
   `Secret`.
3. **Routing guard.** `AnswerType::Secret` cannot be routed to the inquiry
   backend, full stop.
   The coordinator enforces this with two fail-fast checks before routing:
   - No TTY available — fail the tool with a tool-level error and record
     `InquiryResponse::Cancelled { reason: NoPromptBackend }`.
   - Explicit `target = "assistant"` — fail the tool with a tool-level error
     and record `InquiryResponse::Cancelled { reason: AssistantRoutingDenied }`.
     Both checks are generic on `question.answer_type == Secret`.
     The same machinery serves [RFD 083]'s `exclusive: true` flag (083 reuses
     these variants for its routing fail-fast paths).

### `InquiryResponse` serialization

The widened `InquiryResponse` is a Rust enum that makes invalid
(both-or-neither) states unrepresentable at the type level, and carries an
explicit reason on the cancellation variant so replay and debugging can
distinguish each non-answer outcome:

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

`CancellationReason` carries a custom `Serialize`/`Deserialize` pair.
The on-disk shape is always `{"reason": "<tag>"}`: `User` round-trips as
`"user"`, `BackendError` as `"backend_error"`, and `Unknown(s)` round-trips `s`
verbatim.
On deserialization, any tag the build does not recognize lands as `Unknown(tag)`
instead of erroring.
This keeps audit-trail integrity across version boundaries — an older JP
reading a stream written by a newer JP loads the event, marked opaque, rather
than failing the load.

JP itself **never writes `Unknown(_)`**: every variant produced by JP is one
this build knows by name.
`Unknown` is a read-only landing pad for future variants.
Future RFDs that extend the enum add their variants in the same module as
siblings of the existing ones — [RFD 083] contributes `InvalidStaticAnswer` for
a `QuestionConfig.answer` that does not match the in-flight question's
`answer_type` or `options`, and reuses 082's `NoPromptBackend` and
`AssistantRoutingDenied` for its `exclusive: true` routing guards.

`Redacted` carries only `id`: no answer field, no reason field.
It is the canonical shape for "the question was answered and the tool received
the answer, but JP deliberately did not persist it" (see [Secret
questions](#secret-questions)).

Default `Serialize` writes the internally-tagged form, consistent with
`InquirySource` in the same module:

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

A custom `Deserialize` for `InquiryResponse` accepts both the new tagged form
*and* the legacy flat form written by pre-082 code:

```json
{
  "id": "call_1.answer",
  "answer": true
}
```

Legacy events without an `outcome` field deserialize as
`InquiryResponse::Answered`.
The absence of `outcome`, `answer`, and a `Redacted`-shaped payload is a
deserialization error — no legacy cancellations or redactions exist because
neither variant existed before this RFD.
When `outcome == "cancelled"` is present but the `reason` field is missing, the
custom `Deserialize` defaults the reason to `CancellationReason::User` — the
conservative choice, since the other known variants (`BackendError`, `Unknown`)
require JP itself to have produced the event.

This is the standard "write new, read both" backward-compat pattern.
Old conversation streams continue to read correctly; every new write produces
the tagged form.
No migration step is required — the on-disk shape heals one event at a time as
new responses are recorded.

### Reader behavior

The widened `InquiryResponse` enum changes what existing readers of the
persisted stream see.
Defined behavior:

- **Markdown export** (`crates/jp_cli/src/editor.rs`):
  - `Answered { answer }`: `Answer: <value>` (unchanged).
  - `Redacted { id }`: `Answer: <redacted>`.
  - `Cancelled { reason }`: `Cancelled (<reason>)` where `<reason>` is the
    serde-encoded tag of the variant (`user`, `backend_error`,
    `no_prompt_backend`, `assistant_routing_denied`, or — for `Unknown(tag)` —
    the unparsed `tag` preserved verbatim from the source stream).
    The implementation should derive this from the variant's serde
    representation so new `CancellationReason` variants render correctly without
    code changes; if that requires a small helper (e.g.
    `CancellationReason::as_str`), add it alongside the enum.
- **`jp conversation grep`** (`crates/jp_cli/src/cmd/conversation/grep.rs`):
  continues to ignore `InquiryResponse` entirely.
  The response side has no greppable text; changing this is out of scope.
- **Turn renderer** (`crates/jp_cli/src/render/turn.rs`): continues to skip
  inquiry events entirely.
  Pretty rendering for `jp conversation show` is a non-goal here (see
  [Non-Goals] (\#non-goals)).

Any other reader that pattern-matches on `InquiryResponse` MUST handle all
variants.
The default `serde` deserializer can now produce values across the full enum,
not just `Answered`.

### Splitting the turn-cache state

Today's `TurnState.persisted_inquiry_responses: IndexMap<InquiryId,
InquiryResponse>` serves two unrelated callers under one name:

- Tool-question answers, keyed `"<tool_name>.<question_id>"`, written and read
  by `handle_tool_result` / `handle_prompt_answer`.
- Tool-permission decisions, keyed `"<tool_name>.__permission__"`, written and
  read by `decide_permission` / `apply_permission_result`.
  Reuse is governed by the permission prompt's `persist` flag.

With the widened `InquiryResponse` (which now also encodes `Cancelled`), storing
`InquiryResponse` values directly conflates audit-log records with
reusable-answer cache entries: a `Cancelled` is never a valid remembered answer
for either caller.
The name `persisted_inquiry_responses` also becomes a misnomer once the values
are no longer `InquiryResponse`s.

Split `TurnState` into two narrowly-named maps:

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

The cache-key shapes (`<tool_name>.<question_id>` and
`<tool_name>.__permission__`) are deliberately distinct from the
stream-correlation `InquiryId` format defined in [Inquiry ID format]
(\#inquiry-id-format) — today's `TurnState` uses `InquiryId` for both, which
blurs the two roles.
Implementation may introduce dedicated newtypes per map to lift this distinction
into the type system; 082 does not require it.

Each caller owns one map; neither needs to know the other exists.
Permission reads use the prompt's `persist` flag, unchanged from today.
Tool-answer reads follow today's behavior; [RFD 083] later adds a persistence
predicate on top of this split when it introduces the `Question.persistence`
field.

Migrating off `InquiryResponse` values lets us write `Cancelled` to the
persisted stream without ever risking a `Cancelled` being read back as a
remembered answer.

### Pre-seed removal

Today the coordinator pre-seeds `accumulated_answers` from
`static_answers_for_all_questions` before tool execution (currently around
`crates/jp_cli/src/cmd/query/tool/coordinator.rs:805`).
For static-answered questions, this means the tool never emits
`Outcome::NeedsInput` — so the coordinator has no question metadata to record
an `InquiryRequest` from.

The pre-seed is removed.
Static answers flow through the existing late-path `static_answer` lookup in
`handle_tool_result` (currently around `coordinator.rs:1323`), which already
handles applying the configured answer once `NeedsInput` fires.
Per-question cost: one extra tool invocation per configured static answer (the
tool's first call returns `NeedsInput`; the coordinator immediately respawns it
with the answer injected).
No prompt is shown, no LLM round-trip happens.
The tradeoff buys uniform recording: every question/answer exchange produces an
`InquiryRequest`/`InquiryResponse` pair regardless of how the answer is
resolved.

**This is a behavior change for user-supplied local tools.** Built-in and MCP
tools are unaffected (built-ins handle `answers` through the explicit two-call
`NeedsInput → Success` pattern; MCP tools never see `answers`).
Local tools receive the accumulated answers in the JSON context under `"tool": {
"answers": answers, ... }` (`crates/jp_llm/src/tool.rs` around line 719).
Today, `QuestionConfig.answer` is documented as "the question will not be
presented to the target, but will always be answered with the given value"
(`crates/jp_config/src/conversation/tool.rs:1240-1241`), and a local tool can
rely on that answer being present on the first invocation.
After 082, a static answer is delivered on the second invocation — the local
tool must emit `NeedsInput` for that question before the static answer flows in.

Local tools that already use the standard `NeedsInput → Success` pattern see no
behavior change.
Local tools that today inspect `tool.answers` on first call and proceed without
emitting `NeedsInput` need to switch to the two-call pattern.
Tools with side effects between argument validation and the first `NeedsInput`
will run those side effects once more than before per configured static answer;
for side-effecting tools this is a real cost, not a transparent refactor.
`QuestionConfig.answer`'s documentation will be updated to reflect the new
contract when this RFD lands.

### Coordinator plumbing

The prompter-answered path currently does not hold a write handle to the
conversation stream.
`handle_prompt_answer` and `handle_prompt_cancelled` gain a `conv:
&ConversationMut` parameter (mirroring `handle_tool_result`, which already
receives it).

The `InquiryId` for each prompt is **allocated once** at the recording site
(when the coordinator records `InquiryRequest`, where the turn-scoped counter is
incremented) and threaded through the prompt-event types so the answer and
cancellation handlers can write the matching `InquiryResponse` without
reconstructing the ID:

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

Today `PromptAnswer` carries `{ index, question_id, answer, persist_level }` and
`PromptCancelled` carries only `{ index }`.
Both gain `inquiry_id` for the recording write.
`PromptAnswer` retains `question_id` because the coordinator continues to key
`ExecutingTool::accumulated_answers` by `question.id`, independent of the
inquiry ID.
`PromptCancelled` needs no extra fields beyond `inquiry_id` — the turn-scoped
counter is incremented at request time, so cancellation does not have to update
it.

Rebuilding the inquiry ID from its constituent parts in the answer/cancellation
handler would require the handler to know the ID format, which conflicts with
the "treat `InquiryId` as opaque" rule in [Inquiry ID
format](#inquiry-id-format).

## Drawbacks

- **Stream-size growth.** Adds two `InquiryRequest`/`InquiryResponse` events per
  tool-question round-trip on the prompter path (which today produces zero).
  Each event is small (the question text plus a JSON answer value), but tools
  with many interactive prompts over many turns accumulate them.
  Bounded by the number of questions a tool actually asks; no growth for tools
  that don't ask questions.
- **Static-answer contract change for user-supplied local tools.** Removing the
  pre-seed optimization means tools with a configured static answer execute
  twice (emit `NeedsInput`, then receive the injected answer) instead of once.
  JP's own built-in and MCP tools are unaffected, but local tools that today
  read `tool.answers` on first invocation and proceed without emitting
  `NeedsInput` will no longer see the static answer on that first call — a real
  contract change for that authoring pattern.
  Tools with side effects before their first `NeedsInput` will run those side
  effects one extra time per configured static answer.
  See the [Pre-seed removal] (\#pre-seed-removal) subsection for migration
  guidance.
- **Touches existing tool-question call sites.** The unified recording affects
  any tool that emits `Outcome::NeedsInput`, not just `ask_user`.
  Each existing caller needs a regression pass.
- **Widening `InquiryResponse` touches every reader of the persisted stream.**
  The custom `Deserialize` for `InquiryResponse` preserves backward compat with
  pre-082 events, but readers that pattern-match on the enum need updating to
  handle the `Cancelled` and `Redacted` variants.
- **`InquirySource` plumbed through `ExecutorResult::NeedsInput`.** Widening the
  executor-result type to carry resolved source metadata touches every
  `Executor` implementation, including `MockExecutor` and any test fixtures that
  construct `ExecutorResult` directly.
  The benefit is that source resolution lives in one place rather than at every
  recording site.
- **`AnswerType` gains a `Secret` variant and a serde representation change.**
  Both `jp_tool::AnswerType` and
  `jp_conversation::event::inquiry::InquiryAnswerType` need the new variant.
  Downstream `match`es on the enum (the prompter, the editor renderer, any code
  that branches on answer types) need a new arm.
  `jp_tool::AnswerType` also gains `#[serde(tag = "type", rename_all =
  "snake_case")]` to align its wire shape with the already-internally- tagged
  `InquiryAnswerType` (and with this RFD's local-tool wire examples).
  The persisted shape for non-secret questions is unchanged.
  Existing in-tree tools build `Question` values through `jp_tool`'s
  constructors rather than hand-encoding JSON, so the wire-shape change is
  transparent to them.
  Old streams cannot contain `Secret` answers, so backward compat on read is
  trivial.
- **`PromptBackend` gains a no-echo input method.** A new method on the trait
  (e.g. `password`) requires updates to `TerminalPromptBackend` (via
  `inquire::Password`) and `MockPromptBackend`, plus any other in-tree
  implementors.
  Small surface but a real trait change.

## Alternatives

### Skip recording, keep [RFD 005]'s exception

Keep [RFD 005]'s carve-out for prompter-answered questions and record only the
`ToolCallRequest`/`ToolCallResponse` pair.
Rejected: the archival value of question/answer exchanges is the same regardless
of whether the user or the inquiry backend answered.
Leaving the prompter path off the stream freezes an implementation gap into the
data model and forces every future feature that wants Q\&A visibility to
re-derive the gap.

### Record only when source is `Assistant`

Closer to a status quo path: record for the inquiry backend (current [RFD 005])
and additionally when the source is `Assistant` (i.e. for `ask_user`), but leave
ordinary user-prompter-answered tool questions off the stream.
Rejected: gates a generic recording mechanism on a discriminator that has
nothing to do with whether the event has archival value.
Keeps the three-path table ([RFD 005]'s) that this RFD is trying to collapse,
and burdens every future RFD that wants stream-level Q\&A visibility with the
same conditional.

### Derive `InquirySource` from `QuestionConfig`

Earlier versions of [RFD 083] proposed `QuestionConfig.attribution`, a
user-overridable enum whose value drove both the prompt label and the persisted
source.
Rejected: provenance becomes a styling option.
A local config bundle could make any tool's questions persist as
assistant-sourced, which defeats the audit-trail purpose [RFD 005] motivates
inquiry events with.
The `BuiltinTool` trait hook keeps the value compile-time-set with no user
override path.

### Defer request/response uniqueness to a stream-entry-level ID

Drop pair-correlation responsibility from `InquiryRequest.id` and rely on a
separate stream-entry-level identifier (a per-event UUID/ULID or similar
primitive) for uniqueness.
`InquiryRequest.id` would carry only the tool-question shape
(`<tool_call_id>.<question_id>`) and the matching response would be paired by
stream order.
Rejected: pair correlation and stream-entry uniqueness are different contracts.
A reader keying by `InquiryRequest.id` should not need to reconstruct pair
semantics from order — that's brittle under manual edits, parallel tool calls,
and any future repair pass.
Encoding the attempt into the ID itself (see [Inquiry ID
format](#inquiry-id-format)) keeps the correlation contract self-contained and
removes the cross-RFD dependency that an external primitive would introduce.

### Use unique per-event `InquiryRequest.id` with backend-supplied uniqueness

Generate IDs from a UUID/ULID source instead of the
`<tool_call_id>.<question_id>.<attempt>` shape.
Rejected: structured IDs make manual reading, hand-edits, and grep useful;
opaque IDs lose that.
The attempt-counter approach gives uniqueness *and* readability, with the
counter bounded per-turn rather than universally.

## Non-Goals

- **Rendering inquiry events in `jp conversation show`.** Display formatting for
  inquiry events in the CLI is deferred, consistent with [RFD 005]'s own
  Non-Goals.
- **Inquiry events for permission and result-delivery prompts.** This RFD
  unifies recording only for the existing inquiry shape (tool questions).
  `RunTool` and `DeliverToolResult` prompts are not inquiry events today and
  remain outside the scope of this RFD.
- **Stream replay and event-driven UI.** Several future features (sub-agent
  reasoning trails, replay tooling, conversation viewers) benefit from this
  RFD's completeness guarantee, but their implementations are their own
  concerns.
- **Migration tooling for legacy streams.** The custom `Deserialize` heals
  legacy events one at a time as new responses are written; no separate
  migration step is provided.

## Risks and Open Questions

- **Substantive amendment to [RFD 005].** 082 changes the "Recording Inquiry
  Events" subsection of RFD 005 — specifically, removes the "prompter-answered
  questions are not recorded" exception, updates the table, and adds a paragraph
  on cancellation.
  This amendment lands bundled with 082 implementation.
- **083 is a downstream consumer, not a dependency.** 082 ships against current
  types without waiting on [RFD 083]. 083 in turn requires 082 — it adds the
  `exclusive: bool` and `persistence: AnswerReusePolicy` fields on
  `jp_tool::Question` (along with the cache-persistence gate and the `exclusive`
  fail-fast routing checks in the coordinator), widens `InquiryQuestion` with
  `context`/`exclusive`/`persistence`, registers `ask_user`'s
  `InquirySource::Assistant` override on `BuiltinTool::inquiry_source()`, and
  adds one new `CancellationReason` variant: `InvalidStaticAnswer` (a configured
  `QuestionConfig.answer` that does not match the in-flight question's
  `answer_type` or `options`). 082's enum ships with `User`, `BackendError`,
  `NoPromptBackend`, `AssistantRoutingDenied`, and the `Unknown(String)`
  read-only landing pad. 083 reuses 082's `NoPromptBackend` and
  `AssistantRoutingDenied` for its `exclusive: true` routing fail-fast paths —
  the guard machinery is shared.
  If 083's field names or defaults change before its own merge, that evolution
  is internal to 083 and does not affect 082.
- **Sensitive data exposure.** 082's `AnswerType::Secret` variant covers the
  common no-persistence case (passwords, passphrases, API keys); tool authors
  opt in by declaring the question's answer type as `Secret`.
  The persisted response is `Redacted` rather than `Answered`, the prompter does
  not echo input, and routing to the inquiry backend (no-TTY fallback or `target
  = "assistant"`) is refused with a tool-level error. 082 does not cover every
  shape of sensitive content — e.g. a text answer that happens to contain a
  token without the question being declared `Secret`, or partial sensitivity
  inside an otherwise-archival answer.
  A future RFD may add richer redaction policies if real-world use surfaces
  them.
- **Stream-size growth for noisy tools.** Tools that prompt frequently produce
  more persisted bytes after this RFD.
  The events are small and the growth is bounded, but conversations dominated by
  interactive workflows pay the cost.

## Implementation Plan

### Phase 1: `BuiltinTool::inquiry_source()` hook and executor-boundary plumbing

Add the `BuiltinTool::inquiry_source(&self, name: &str) -> InquirySource` trait
method with the default `Tool { name }` implementation.
No overrides yet — every built-in inherits the default.

Widen `ExecutorResult::NeedsInput` with a `source: InquirySource` field.
Update `ToolExecutor::execute` to populate it from
`BuiltinTool::inquiry_source()` for `ToolSource::Builtin { .. }` and from
`InquirySource::Tool { name }` for the other two source kinds.
Update `MockExecutor` and `TestExecutorSource` fixtures to set the field.
The coordinator's recording site reads from `ExecutorResult::NeedsInput.source`
instead of the hardcoded `InquirySource::tool(name)` construction.

No behavioral change at this phase (the resolved source returns the same value
the hardcoded call did for every existing tool).

Can be merged independently.

### Phase 2: Widen `InquiryResponse` and split `TurnState`

Add the `Cancelled` variant with `CancellationReason::{User, BackendError,
NoPromptBackend, AssistantRoutingDenied, Unknown(String)}` and the `Redacted`
variant.
Implement the custom `Serialize`/`Deserialize` for `CancellationReason` so
unrecognized tags round-trip as `Unknown(s)` preserving `s` verbatim.
Implement the custom `Deserialize` for `InquiryResponse` that accepts the legacy
untagged form as `Answered` and defaults a missing `reason` on a tagged
`Cancelled` event to `User`.

Split `TurnState.persisted_inquiry_responses` into `remembered_tool_answers:
IndexMap<String, Value>` and `remembered_permission_decisions: IndexMap<String,
Value>`.
Update `handle_tool_result` / `handle_prompt_answer` to consume the tool-answer
map, and `decide_permission` / `apply_permission_result` to consume the
permission-decision map.
Neither caller touches the other map.
The key type is intentionally not `InquiryId` — the cache-key shapes
(`<tool_name>.<question_id>` and `<tool_name>.__permission__`) differ from the
stream-correlation `InquiryId` format (see [Splitting the turn-cache
state](#splitting-the-turn-cache-state)).

Tests:

- **Backward-compat round-trip.** Deserialize a pre-082 answered-response JSON
  and assert it lands as `Answered`.
- **Tagged serialization.** Serialize an `Answered`, a `Cancelled { reason: User
  }`, a `Cancelled { reason: BackendError }`, and a `Redacted`, and assert each
  produces its distinct tagged shape.
- **Missing `reason` defaults to `User`.** Deserialize a tagged-cancelled event
  with no `reason` field and assert it lands as `Cancelled { reason: User }`.
- **Unknown `reason` round-trips through `Unknown(String)`.** Deserialize a
  cancelled event with `"reason": "some_future_variant"` and assert it lands as
  `Cancelled { reason: Unknown("some_future_variant".into()) }`; re-serialize
  and assert the on-disk shape is identical to the input.
- **`Redacted` carries no answer.** Serialize a `Redacted` and assert the JSON
  has no `answer` field; deserialize a `Redacted` and assert no answer is
  materialized.
- **Invalid shape.** Assert that deserializing an event with no `outcome`, no
  `answer`, and no `Redacted`-shape payload is a deserialization error.
- **Turn-cache split.** Round-trip a permission decision through
  `remembered_permission_decisions` and a tool answer through
  `remembered_tool_answers`; assert that neither caller sees the other map's
  entries and that `Cancelled` and `Redacted` inquiry responses never land in
  either map.

Depends on Phase 1.

### Phase 3: Unified recording lifecycle

Recording-lifecycle plumbing:

- Remove the pre-execution seeding of `accumulated_answers` from
  `static_answers_for_all_questions`.
- Thread `&ConversationMut` into `handle_prompt_answer` and
  `handle_prompt_cancelled`.
- Allocate the `InquiryId` at the request-recording site and thread it through
  `PendingPrompt::Question`, `ExecutionEvent::PromptAnswer`, and
  `ExecutionEvent::PromptCancelled` so the answer and cancellation handlers can
  write the matching response without reconstructing the ID (see [Coordinator
  plumbing](#coordinator-plumbing)).
- Record `InquiryRequest` before any routing decision (including cached and
  static answer short-circuits).
- Record `InquiryResponse::Answered` on cached, static, prompter, and
  inquiry-backend success paths (non-`Secret` answer types).
- Record `InquiryResponse::Cancelled` with the appropriate reason per the
  mapping table in [Recording lifecycle](#recording-lifecycle) (`User` for user
  cancellation including `InquiryError::Cancelled`; `BackendError` for genuine
  backend failures; `NoPromptBackend`/`AssistantRoutingDenied` for the
  secret-routing guard paths).

Inquiry ID format:

- Add a per-`(tool_call_id, question_id)` attempt counter to `TurnState`.
  Increment at the `InquiryRequest`-recording site whenever a question is
  recorded under that key; first recording is attempt `1`.
  The counter resets only at turn boundaries (i.e. when a fresh `TurnState` is
  built).
- Construct `InquiryRequest.id` as `<tool_call_id>.<question_id>.<attempt>` at
  the recording site.
  The matching `InquiryResponse.id` uses the same value.
- Update the pairing logic in any reader that keys by `InquiryId` to accept both
  legacy two-segment and new three-segment IDs (legacy duplicates fall back to
  request/response order; new writes are unique by construction).
- Make inquiry-orphan repair in `ConversationStream::sanitize()` turn-scoped.
  Today both `remove_orphaned_inquiry_responses` and
  `remove_orphaned_inquiry_requests` build flat ID sets across the whole stream
  (`crates/jp_conversation/src/stream.rs` around
  `remove_orphaned_inquiry_responses`), which can cross-satisfy pairs across
  turns once `tool_call_id` (and therefore `InquiryId`) reuse becomes possible.
  Orphan removal and any synthetic-response injection operate within each turn's
  event window instead.
  The same change applies to any other code that builds an `InquiryId` index
  over the full stream.

Secret-question handling:

- Add a `Secret` variant to `jp_tool::AnswerType` (the existing
  `Boolean`/`Select`/`Text` enum) and add `#[serde(tag = "type", rename_all =
  "snake_case")]` to the enum so its wire shape (`{"type": "boolean"}`,
  `{"type": "select", "options": [...]}`, `{"type": "text"}`, `{"type":
  "secret"}`) matches `InquiryAnswerType`.
  Existing in-tree tools build `Question` values through `jp_tool`'s
  constructors, so the wire-shape change is transparent.
  Add a matching `Question::secret(text: String) -> Self` constructor in
  `jp_tool` alongside `Question::text`/`boolean`/`select`.
  No `secret: bool` field is added to `Question`.
- Add the matching `Secret` variant to
  `jp_conversation::event::inquiry::InquiryAnswerType` (which is already
  serialized via `tag = "type"`, `rename_all = "snake_case"`, so the on-disk
  shape is `{"type": "secret"}`).
  Propagate the variant at the recording boundary
  (`tool_question_to_inquiry_question` or equivalent).
- Add a no-echo input method on `PromptBackend` (e.g. `password`).
  Implement it on `TerminalPromptBackend` via `inquire::Password` and on
  `MockPromptBackend` as a regular queued response source.
  Update any other in-tree implementors.
- In `ToolPrompter::prompt_question`, dispatch to the no-echo path when
  `question.answer_type == AnswerType::Secret`.
  The variant is text-shaped by construction; no special handling is needed for
  booleans/selects.
- In the cached-answer short-circuit, skip the lookup and the write for `Secret`
  questions.
  In the static-answer short-circuit, apply the configured value to the
  in-memory `accumulated_answers` but record `InquiryResponse::Redacted { id }`
  instead of `Answered`.
  On a successful prompter or inquiry-backend answer for a `Secret` question,
  record `InquiryResponse::Redacted { id }` in place of `Answered`.
- Enforce the routing guard before routing a `Secret` question:
  - **No TTY available**: synthesize a tool-level error response and record
    `InquiryResponse::Cancelled { reason: NoPromptBackend }`.
  - **Explicit `target = "assistant"`**: synthesize a tool-level error response
    and record `InquiryResponse::Cancelled { reason: AssistantRoutingDenied }`.
    Both checks are generic on `question.answer_type == Secret`.
    The same machinery serves [RFD 083]'s `exclusive: true` flag.

Tests:

- **Attempt counter.** A tool that emits the same `question_id` twice within a
  single tool call produces two distinct `InquiryRequest.id` values (`.1`, `.2`)
  and two correctly-paired `InquiryResponse` events.
- **Cross-cycle uniqueness within a turn.** A turn where the same `tool_call_id`
  is reused across cycles (Gemini-style) and the same `question_id` is emitted
  in each produces two distinct `InquiryRequest.id` values (`.1`, `.2`) — the
  counter continues rather than restarting.
- **Per-turn reset.** Two separate turns both start the counter at `1` for any
  `(tool_call_id, question_id)` key; counters do not persist across turns.
- **Legacy ID pairing.** A pre-082 stream with two-segment IDs reads back with
  its request/response pairs intact, including the case where two legacy events
  share an ID and pair by order.
- **Secret prompter answer.** A built-in tool that emits a `Question {
  answer_type: AnswerType::Secret, .. }` answered at the prompter produces an
  `InquiryResponse::Redacted` event; the answer is delivered to the tool
  in-memory; no answer value appears in the persisted stream.
- **Secret static answer.** A `Secret` question with a configured
  `QuestionConfig.answer` is delivered to the tool in-memory and records as
  `InquiryResponse::Redacted` (not `Answered`).
- **Local-tool secret.** A local tool that emits `{"type": "needs_input",
  "question": {"answer_type": {"type": "secret"}, ...}}` on stdout produces an
  `InquiryResponse::Redacted` event; the answer is delivered to the tool's next
  invocation in the `tool.answers` JSON context; no answer value appears in the
  persisted stream.
- **Cache bypass.** A `Secret` question is not stored in
  `remembered_tool_answers` (write side), and a pre-existing entry in the cache
  whose question is now `Secret` is bypassed (read side).
- **Non-secret regression.** A non-secret question (any of `Boolean`, `Select`,
  or `Text`) is recorded as `Answered` and continues to flow through the cache
  as it does today.
- **Inquiry ID round-trip via prompt events.** An `InquiryId` allocated at the
  request-recording site survives an `ExecutionEvent::PromptAnswer` round-trip
  verbatim; the response written by `handle_prompt_answer` carries the same ID
  as the request.
- **Cancellation reason mapping.** `InquiryError::Cancelled` (cancellation token
  fired) lands as `Cancelled { reason: User }`; `InquiryError::Provider`,
  `MissingStructuredData`, `AnswerExtraction`, and `Other` each land as
  `Cancelled { reason: BackendError }`.
- **Cross-turn ID collision under sanitize.** Build a stream where an orphaned
  `InquiryRequest` in turn 1 shares its ID with a valid request/response pair in
  turn 2 (`tool_call_id` reuse across turns with the turn-scoped attempt counter
  restarting at `1`); assert `ConversationStream::sanitize()` repairs the turn-1
  orphan within its own turn rather than cross-satisfying it from turn 2.
- **Legacy duplicate IDs within a turn under sanitize.** Build a turn with two
  `InquiryRequest`s sharing a legacy two-segment ID and one matching response;
  assert `sanitize()` preserves the by-order matched pair and repairs the
  remaining orphan within the turn.
- **Secret routing guard, no TTY.** A `Secret` question encountered without a
  TTY fails the tool with a tool-level error response and records `Cancelled {
  reason: NoPromptBackend }`.
- **Secret routing guard, assistant target.** A `Secret` question with `target =
  "assistant"` fails the tool with a tool-level error response and records
  `Cancelled { reason: AssistantRoutingDenied }`.

The `Assistant`-attribution code path is exercised with a synthetic test tool
that overrides `BuiltinTool::inquiry_source()`.
The real consumer (`ask_user`) arrives in [RFD 083], which builds on 082 to
register its override and to widen `InquiryQuestion` with its source-side
fields.

Depends on Phases 1 and 2.

### Phase 4: Amend [RFD 005]

Remove the "prompter-answered questions are not recorded" exception in RFD 005's
"Recording Inquiry Events" subsection.
Update the routing table.
Add a paragraph on cancellation.

Independent of the others, lands alongside Phase 3 in the same PR.

## References

- [RFD 005] — first-class inquiry events; substantively amended by this RFD to
  extend recording to prompter-answered questions and to add the `Cancelled`
  variant.
- [RFD 023] — resumable conversation turns; downstream consumer of 082.
  RFD 023 assumes user-targeted `InquiryRequest`/`InquiryResponse` pairs are on
  disk so incomplete turns blocked on inquiries can be detected and resumed.
  RFD 005's prompter carve-out leaves that assumption unmet; 082 closes the gap.
  RFD 023's `Requires` needs updating to reference 082 at 082 promotion time.
- [RFD 028] — the structured inquiry system this RFD's recording layer builds
  on.
- [RFD 034] — defines the current `QuestionTarget` shape.
- [RFD 083] — the `ask_user` tool that motivated this RFD; consumes 082's
  recording infrastructure to register an `InquirySource::Assistant` override on
  `BuiltinTool::inquiry_source()`, widens `InquiryQuestion` with
  `context`/`exclusive`/`persistence`, and contributes
  `CancellationReason::InvalidStaticAnswer` for the static-answer validation it
  introduces. 083 reuses 082's `NoPromptBackend` and `AssistantRoutingDenied`
  for its `exclusive: true` routing fail-fast paths. 083 ships on top of 082
  (083 `Requires` 082); 082 does not depend on 083.
- [RFD 049] — defines the full `exclusive` flag and detached-policy cascade;
  consumed downstream by [RFD 083].

[RFD 005]: 005-first-class-inquiry-events.md
[RFD 005-2]: ./005-first-class-inquiry-events.md
[RFD 023]: 023-resumable-conversation-turns.md
[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 034]: 034-inquiry-specific-assistant-configuration.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 083]: 083-built-in-ask_user-tool-for-assistant-initiated-inquiries.md
