# RFD 071: Inquiry Rejection Reasoning and Escalation

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-14
- **Extends**: [RFD 028](028-structured-inquiry-system-for-tool-questions.md),
  [RFD 034](034-inquiry-specific-assistant-configuration.md)

## Summary

This RFD adds mandatory reasoning to assistant-answered inquiries and introduces
an escalation mechanism that routes rejected inquiries to the user instead of
failing the tool. Together these address two observed failure modes: the main
model spiraling after opaque sub-agent rejections, and sub-agent misjudgments
causing unnecessary tool failures.

## Motivation

When a tool needs confirmation (e.g. `fs_modify_file`'s `apply_changes`
question), the inquiry system ([RFD 028]) routes the question to a sub-agent.
If the sub-agent answers `false`, the tool fails with:

```
`apply_changes` inquiry was answered with `false`. Changes discarded.
```

This message is problematic on three levels:

1. **Opaque to the main model.** The main model does not know *who* answered the
   question, *why* they rejected it, or that it was a sub-agent decision rather
   than a user rejection. The term "inquiry" is internal jargon with no meaning
   to the model.

2. **No reasoning trail.** The inquiry system extracts a raw `Value` from the
   sub-agent's structured response. There is no mechanism for the sub-agent to
   explain its decision. The main model cannot distinguish a deliberate
   rejection ("the change is incorrect") from a misjudgment ("I didn't
   understand the diff").

3. **No recovery path.** The tool fails, and the main model's only options are
   to retry the identical call (same arguments, same sub-agent, same result) or
   work around it with destructive alternatives (deleting and recreating files).
   In practice, the main model often spirals — retrying multiple times, then
   falling back to `fs_delete_file` + `fs_create_file`, which loses the
   confirmation safeguard entirely.

Observed consequences in production:

- A main model (Opus) attempted 13 RFD cross-reference additions. The sub-agent
  (Haiku) rejected the first two, then accepted identical changes on later
  retries. The main model wasted turns reasoning about why its changes were
  being rejected.

- A main model tried to modify a file three times. After repeated rejections, it
  deleted and recreated the file from scratch — bypassing the review mechanism
  the inquiry was designed to provide.

## Design

### Overview

Two changes, designed to be implemented independently:

1. **Reasoning in inquiry responses.** The structured output schema gains a
   `reason` field. The sub-agent must explain its answer before committing to
   it. The reason is threaded through the error path so the main model receives
   actionable context.

2. **Escalation target.** A new `QuestionTarget` variant routes the question to
   the sub-agent first, then escalates to the user if the sub-agent rejects.
   This preserves the speed advantage of sub-agent approval while providing a
   safety net for misjudgments.

### Part 1: Reasoning in Inquiry Responses

#### Schema change

The structured output schema gains a required `reason` field, ordered before
`answer` so the sub-agent performs chain-of-thought before committing to a
decision (field order matters for autoregressive generation):

Current:
```json
{
  "type": "object",
  "properties": {
    "answer": { "type": "boolean" }
  },
  "required": ["answer"],
  "additionalProperties": false
}
```

Proposed:
```json
{
  "type": "object",
  "properties": {
    "reason": {
      "type": "string",
      "description": "Brief explanation of why you chose this answer."
    },
    "answer": { "type": "boolean" }
  },
  "required": ["reason", "answer"],
  "additionalProperties": false
}
```

The `reason` field is stable across inquiries of the same answer type. This
preserves the cache stability property from [RFD 034] — the schema does not
change between inquiries.

#### `InquiryResponse` extension

Add an optional `reason` field:

```rust
pub struct InquiryResponse {
    pub id: InquiryId,
    pub answer: Value,
    /// Explanation provided by the responder for their answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
```

Backward-compatible: existing serialized responses without `reason` deserialize
with `None`.

#### `InquiryBackend` return type

The `InquiryBackend::inquire` method currently returns `Result<Value, InquiryError>`.
Change the success type to carry the reason:

```rust
pub struct InquiryAnswer {
    pub value: Value,
    pub reason: Option<String>,
}
```

The `LlmInquiryBackend` extracts both fields from the structured response. The
`MockInquiryBackend` returns `None` for reason by default.

#### Threading reason into tool error messages

When the coordinator receives an `InquiryResult` with a successful answer, it
stores the reason in the `InquiryResponse` event. When the tool subsequently
fails because the answer was a rejection (e.g. `apply_changes = false`), the
error response needs the reason.

Two approaches:

**Option A: Pass reason through `accumulated_answers`.** Extend the answers map
to carry metadata alongside values. This is invasive — it changes the tool
contract.

**Option B: Coordinator intercepts rejections.** For boolean inquiries where
the sub-agent answers `false`, the coordinator synthesizes the error response
directly instead of re-executing the tool. The tool never sees `false`.

**Option C: Enrich the tool error message in the coordinator.** The coordinator
re-executes the tool as today. When the tool returns an error, and the most
recent inquiry for that tool had a `false` answer with a reason, the coordinator
replaces the tool's error message with a richer one that includes the reason and
attribution.

Option C is the least invasive. The tool's error message is an implementation
detail that the coordinator already wraps in a `ToolCallResponse`. The enriched
message would look like:

```
The proposed changes to 'docs/rfd/008-knowledge-base.md' were reviewed by a
secondary assistant (claude-haiku-4-5) and rejected.

Reason: "The TIP admonition references RFD 016, but the surrounding section
discusses knowledge base architecture. The cross-reference appears tangential
to the current content."

The changes were not applied. You may retry with different changes or ask the
user to review.
```

The coordinator can detect this case by tracking which tools had sub-agent
answered inquiries in the current execution cycle. When a tool error follows
a `false` boolean inquiry answer, the coordinator enriches the error.

#### Prompt text adjustment

The inquiry question sent to the sub-agent should instruct it to provide
reasoning. The current prompt format in `LlmInquiryBackend::inquire`:

```
The tool `fs_modify_file` requires additional input.

Do you want to apply the following patch?

<patch content>

Provide your answer based on the conversation context.
```

Add an instruction for the reason field:

```
The tool `fs_modify_file` requires additional input.

Do you want to apply the following patch?

<patch content>

Provide your answer based on the conversation context. In the `reason` field,
briefly explain why you chose your answer.
```

### Part 2: Escalation

#### New `QuestionTarget` variant

```rust
pub enum QuestionTarget {
    /// Route the question to the interactive user.
    User,

    /// Route the question to the assistant (sub-agent). On rejection, the tool
    /// fails.
    Assistant(Box<PartialAssistantConfig>),

    /// Route the question to the assistant first. If the assistant rejects,
    /// escalate to the interactive user. If no interactive user is available,
    /// follow the detached prompt policy.
    AssistantWithEscalation(Box<PartialAssistantConfig>),
}
```

#### Configuration

String shorthand:
```toml
questions.apply_changes.target = "assistant_with_escalation"
```

Map form (with per-question model override):
```toml
[conversation.tools.fs_modify_file.questions.apply_changes.target]
model.id = "anthropic/claude-haiku-4-5"
escalation = true
```

The `escalation` field in the map form distinguishes `Assistant` from
`AssistantWithEscalation`. When `escalation` is absent or `false`, the target
is `Assistant`. When `true`, it is `AssistantWithEscalation`.

Deserialization:

- `"user"` → `User`
- `"assistant"` → `Assistant(default())`
- `"assistant_with_escalation"` → `AssistantWithEscalation(default())`
- `{ model.id = "...", escalation = false }` → `Assistant(config)`
- `{ model.id = "...", escalation = true }` → `AssistantWithEscalation(config)`
- `{ model.id = "..." }` → `Assistant(config)` (escalation defaults to false)

#### Escalation flow

When the coordinator receives an `InquiryResult` with a `false` boolean answer
and the question target is `AssistantWithEscalation`:

1. **Interactive mode (`is_tty`).** The coordinator routes the original question
   to the user via `spawn_user_prompt`, prefixed with the sub-agent's
   recommendation:

   ```
   The assistant recommended rejecting this change:
   "<sub-agent's reason>"

   Do you want to apply the following patch?
   <original patch>
   ```

   The user sees the sub-agent's reasoning and makes the final call. If the user
   approves, the tool proceeds. If the user rejects, the tool fails with a
   user-rejection message (distinct from a sub-agent rejection).

2. **Non-interactive mode.** Escalation follows the detached prompt policy
   ([RFD 049]):
   - `deny`: fail with the sub-agent's rejection (same as `Assistant`).
   - `defaults`: use the question's default value (`true` for `apply_changes`).
   - `auto`: fail (the sub-agent already said no; auto-approving over a
     rejection would undermine the review).

#### When does escalation trigger?

Escalation triggers when the sub-agent's answer would cause the tool to fail.
For the common case (boolean `apply_changes`-style questions), this is a `false`
answer. Generalizing:

| Answer type | Escalation trigger |
|-------------|-------------------|
| Boolean | `false` answer |
| Select | Not applicable (any valid selection proceeds) |
| Text | Not applicable (any text proceeds) |

For boolean questions, the coordinator can detect the rejection before
re-executing the tool. For select and text questions, there's no general concept
of "rejection" — any valid answer proceeds. Escalation is therefore only defined
for boolean inquiries in this RFD.

Future extension: a tool could return `NeedsInput` again after receiving an
answer it considers invalid. The coordinator could detect this re-ask cycle and
escalate. This is left to a future RFD.

#### Coordinator changes

The `handle_tool_result` path for `InquiryResult` currently has two branches:
`Ok(answer)` (re-execute tool) and `Err(error)` (fail tool). Escalation adds
a third path within the `Ok` branch:

```rust
ExecutionEvent::InquiryResult { index, question_id, question_text, result }
=> match result {
    Ok(answer) => {
        let should_escalate = answer.value == Value::Bool(false)
            && matches!(target, QuestionTarget::AssistantWithEscalation(_))
            && is_tty;

        if should_escalate {
            // Route to user with sub-agent context
            let escalated_text = format!(
                "The assistant recommended rejecting:\n\"{}\"\n\n{}",
                answer.reason.as_deref().unwrap_or("(no reason given)"),
                question_text,
            );
            let escalated_question = Question {
                id: question_id,
                text: escalated_text,
                answer_type: AnswerType::Boolean,
                default: Some(Value::Bool(true)),
            };
            Self::spawn_user_prompt(index, escalated_question, prompter.clone(), event_tx);
        } else {
            // Existing path: insert answer, re-execute tool
        }
    }
    Err(error) => { /* existing error path */ }
}
```

The coordinator needs to know the `QuestionTarget` for the tool/question pair.
It already has `question_target()` — the same lookup used when routing the
initial inquiry.

### Improved error message (immediate fix)

Independent of Parts 1 and 2, the error message in `modify_file.rs` should be
improved:

Current:
```rust
"`apply_changes` inquiry was answered with `false`. Changes discarded."
```

Proposed:
```rust
"The proposed file changes were reviewed and rejected. Changes were not applied. \
 You may retry with different changes."
```

This removes internal jargon ("inquiry"), states the outcome clearly, and
suggests a recovery action.

## Drawbacks

- **Reason field adds tokens.** Every inquiry response now includes a reason
  string (typically 1-3 sentences). At ~50-100 tokens per reason on a cheap
  model (Haiku), the cost is negligible per inquiry but adds up for tools that
  ask many questions.

- **Escalation adds latency in the rejection case.** When the sub-agent rejects
  and escalation triggers, the user sees the prompt after the sub-agent's round
  trip (~1-3 seconds on Haiku). This is acceptable — the alternative is a failed
  tool and model spiraling.

- **Config surface grows.** `AssistantWithEscalation` is a third target variant.
  The string shorthand keeps simple cases simple, but the concept of "sub-agent
  with user fallback" needs documentation.

- **Escalation is only defined for boolean inquiries.** Select and text
  questions don't have a clear "rejection" semantic. This is a limitation, not a
  flaw — boolean confirmation is the dominant use case for `apply_changes`-style
  questions.

## Alternatives

### Retry with feedback instead of escalation

Before escalating to the user, retry the inquiry with the main model's original
intent as additional context: "The main assistant intended to add a TIP
admonition. The sub-agent rejected because X. Please reconsider."

Rejected for now: risks infinite loops (sub-agent might reject again for the
same reason), adds complexity, and the failure case (main model spiraling) is
worse than prompting the user. Could be layered on top of escalation as a future
optimization — retry once, then escalate.

### Coordinator intercepts `false` answers

Instead of re-executing the tool with `false`, the coordinator synthesizes the
error response directly. The tool never sees `false`.

Rejected: this changes the tool's contract. The tool's `apply_changes` logic
exists for a reason — other answer sources (user, static config) can also
produce `false`. Intercepting only sub-agent rejections creates a special case
in the coordinator that doesn't generalize.

### Main model explicitly requests escalation

After receiving a rejection error, the main model could call a builtin
`escalate_inquiry` tool to ask the user directly. More general than config-based
escalation but adds a tool call round trip and relies on the model recognizing
when to escalate.

Deferred: could complement config-based escalation as a manual override. Worth
exploring in a future RFD if the automatic escalation proves insufficient.

### `reason` as optional (not required)

Make `reason` optional in the schema, letting the sub-agent skip it. Rejected:
the whole point is chain-of-thought before the answer. Making it optional means
many sub-agents will skip it, especially smaller models. A required field with
a `description` hint produces reliable reasoning.

## Non-Goals

- **Escalation for select/text inquiries.** Only boolean inquiries have a clear
  rejection semantic. Generalized escalation (e.g. tool re-asks after an
  unsatisfactory answer) is future work.
- **Batching inquiry retries.** Retrying a rejected inquiry with additional
  context before escalating. Possible future optimization.
- **User-initiated escalation override.** A mechanism for the main model to
  explicitly escalate to the user. Deferred.
- **Rendering inquiry reasoning in `conversation show`.** Display formatting for
  the reason field in CLI output is deferred.

## Risks and Open Questions

- **Should `assistant_with_escalation` be the default for `fs_modify_file`?**
  Currently `target = "assistant"`. The cost of a false rejection (model spirals,
  deletes files) far exceeds the cost of an occasional user prompt. Recommend
  changing the project default to `assistant_with_escalation`.

- **Reason quality on small models.** Haiku's reasoning may be shallow ("The
  change looks incorrect" without specifics). The `description` hint in the
  schema helps, but quality depends on model capability. Per-question model
  overrides ([RFD 034]) let users route important questions to more capable
  models.

- **Escalation UX.** When escalation triggers, the user sees a prompt they
  didn't expect — the sub-agent was supposed to handle it silently. The prompt
  should clearly indicate why escalation occurred ("The assistant recommended
  rejecting...") so the user understands the context.

- **Interaction with RFD 018 and 049.** Both are in Discussion status.
  Escalation is a routing decision that fits into `route_prompt()` from RFD 018.
  The detached policy from RFD 049 governs escalation in non-interactive mode.
  Implementation should coordinate with those RFDs' timelines.

- **`auto` detached policy and escalation.** When the detached policy is `auto`
  and escalation triggers, should `auto` approve (overriding the sub-agent) or
  fail? This RFD proposes fail — auto-approving over a sub-agent rejection
  defeats the purpose of the review. But this means `auto` behaves differently
  for escalated vs. direct questions, which may be surprising.

## Implementation Plan

### Phase 0: Improve error message

Change the static error string in `modify_file.rs` to a clearer message.

No dependencies. Can be merged immediately.

### Phase 1: Reasoning in inquiry schema

Extend `create_inquiry_schema` to include the `reason` field. Update the prompt
text to instruct the sub-agent to provide reasoning. Update
`LlmInquiryBackend::inquire` to extract both `reason` and `answer`. Add
`InquiryAnswer` return type.

Add `reason: Option<String>` to `InquiryResponse`.

Update the coordinator to store the reason in `InquiryResponse` events and
track the most recent reason per tool for error enrichment.

Depends on Phase 0. Can be merged independently of Phase 2.

### Phase 2: Error message enrichment

When a tool fails after a sub-agent rejection, the coordinator enriches the
error with the sub-agent's reason and model attribution. The main model
receives a structured, actionable error message.

Depends on Phase 1.

### Phase 3: Escalation target

Add `AssistantWithEscalation` to `QuestionTarget`. Implement config
deserialization (string and map forms). Add escalation logic to the
coordinator's `InquiryResult` handler.

Depends on Phase 1 (needs reason for escalation prompt context). Can be merged
independently of Phase 2.

### Phase 4: Default configuration

Change the project default for `fs_modify_file` from `target = "assistant"` to
`target = "assistant_with_escalation"`. Evaluate other tools with boolean
inquiries for the same change.

Depends on Phase 3.

## References

- [RFD 028: Structured Inquiry System][RFD 028] — the inquiry system this
  extends.
- [RFD 034: Inquiry-Specific Assistant Configuration][RFD 034] — per-question
  model overrides and cache optimization.
- [RFD 018: Typed Prompt Routing Enum][RFD 018] — the `Prompt` enum and
  `route_prompt()` function that escalation integrates with.
- [RFD 049: Non-Interactive Mode and Detached Prompt Policy][RFD 049] —
  detached policy governs escalation in non-interactive mode.
- [RFD 005: First-Class Inquiry Events][RFD 005] — persisted inquiry events
  gain the `reason` field.

[RFD 005]: 005-first-class-inquiry-events.md
[RFD 018]: 018-typed-prompt-routing-enum.md
[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 034]: 034-inquiry-specific-assistant-configuration.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
