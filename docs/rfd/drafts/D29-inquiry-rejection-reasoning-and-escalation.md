# RFD D29: Inquiry Rejection Reasoning and Escalation

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-14
- **Extends**: [RFD 028], [RFD 034]
- **Requires**: [RFD 049]
- **Summary**: Requires reasoning on assistant-answered inquiries and escalates
  rejected inquiries to the user instead of failing the tool call.

## Summary

This RFD requires assistant-answered inquiries to include reasoning and adds an
escalation target that routes rejected inquiries to the user instead of failing
the tool call.
Together, these changes address two failure modes: the main model spiraling
after opaque sub-agent rejections, and sub-agent misjudgments causing
unnecessary tool failures.

## Motivation

When a tool needs confirmation (e.g. `fs_modify_file`'s `apply_changes`
question), the inquiry system ([RFD 028]) routes the question to a sub-agent.
If the sub-agent answers `false`, the tool fails with:

```
`apply_changes` inquiry was answered with `false`. Changes discarded.
```

This failure has three problems:

1. **Opaque to the main model.** The message does not state *who* rejected the
   change (a sub-agent or the user) or *why*.
   The term "inquiry" is also internal jargon with no meaning to the model.

2. **No reasoning trail.** The inquiry system extracts only a raw `Value` from
   the sub-agent's structured response, giving the sub-agent no way to explain
   its decision.
   The main model cannot distinguish a deliberate rejection ("the change is
   incorrect") from a misjudgment ("I didn't understand the diff").

3. **No recovery path.** When the tool fails, the main model can only retry the
   identical call (same arguments, same sub-agent, same result) or work around
   the failure with destructive alternatives.
   In practice, the model often retries several times before falling back to
   `fs_delete_file` and `fs_create_file`, losing the confirmation safeguard
   entirely.

Both failure modes have occurred in production:

- A main model (Opus) attempted 13 RFD cross-reference additions.
  The sub-agent (Haiku) rejected the first two, then accepted identical changes
  on later retries.
  The main model wasted turns reasoning about why its changes had been rejected.

- After three rejected attempts to modify a file, a main model deleted and
  recreated the file from scratch, bypassing the review mechanism the inquiry
  provides.

## Design

### Overview

The design has two independent parts:

1. **Reasoning in inquiry responses.** The structured output schema gains a
   `reason` field so the sub-agent must explain its answer before committing to
   it.
   On rejection, the coordinator threads that reason into the tool error message
   to give the main model actionable context.

2. **Escalation target.** A new `QuestionTarget` variant routes the question to
   the sub-agent first and escalates to the user if the sub-agent rejects it.
   This preserves fast sub-agent approval while catching misjudgments.

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

Because the `reason` field does not vary across inquiries of the same answer
type, the schema stays static between inquiries and preserves the prompt-cache
stability from [RFD 034].

#### `InquiryResponse` extension

`InquiryResponse` gains an optional `reason` field:

```rust
pub struct InquiryResponse {
    pub id: InquiryId,
    pub answer: Value,
    /// Explanation provided by the responder for their answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
```

Existing serialized responses without `reason` deserialize with `None`,
preserving backward compatibility.

#### `InquiryBackend` return type

`InquiryBackend::inquire` changes from returning `Result<Value, InquiryError>`
to returning `Result<InquiryAnswer, InquiryError>`, where `InquiryAnswer`
carries the reason alongside the answer value:

```rust
pub struct InquiryAnswer {
    pub value: Value,
    pub reason: Option<String>,
}
```

`LlmInquiryBackend` extracts both fields from the structured response.
`MockInquiryBackend` defaults `reason` to `None`.

#### Threading reason into tool error messages

When the coordinator receives an `InquiryResult` with a successful answer, it
stores the reason in the `InquiryResponse` event.
If the tool subsequently fails because the answer was a rejection (e.g.
`apply_changes = false`), the error response needs that reason.

There are three ways to pass the reason into the error response:

- **Option A: Pass the reason through `accumulated_answers`.** Extend the
  answers map to carry metadata alongside values.
  This is invasive because it changes the tool contract.

- **Option B: Intercept rejections in the coordinator.** For boolean inquiries
  where the sub-agent answers `false`, synthesize the error response in the
  coordinator instead of re-executing the tool.
  The tool never sees `false`.

- **Option C: Enrich the tool error message in the coordinator.** Re-execute the
  tool as usual.
  Track which tools received sub-agent inquiry answers during the current
  execution cycle; when a tool returns an error after a `false` boolean inquiry
  answer with a reason, replace its error message with one that includes the
  reason and model attribution.

This RFD chooses Option C. It leaves the tool contract untouched because the
coordinator already wraps the tool's error string in a `ToolCallResponse`.
The enriched message has this form:

```
The proposed changes to 'docs/rfd/008-knowledge-base.md' were reviewed by a
secondary assistant (claude-haiku-4-5) and rejected.

Reason: "The TIP admonition references RFD 016, but the surrounding section
discusses knowledge base architecture. The cross-reference appears tangential
to the current content."

The changes were not applied. You may retry with different changes or ask the
user to review.
```

#### Inquiry prompt

The prompt constructed in `LlmInquiryBackend::inquire` instructs the sub-agent
to populate the `reason` field.

Current prompt:

```
The tool `fs_modify_file` requires additional input.

Do you want to apply the following patch?

<patch content>

Provide your answer based on the conversation context.
```

Proposed prompt:

```
The tool `fs_modify_file` requires additional input.

Do you want to apply the following patch?

<patch content>

Provide your answer based on the conversation context. In the `reason` field,
briefly explain why you chose your answer.
```

### Part 2: Escalation

#### New `QuestionTarget` variant

`QuestionTarget` gains an `AssistantWithEscalation` variant:

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

In the map form, the `escalation` field selects between `Assistant` (when
`false` or absent) and `AssistantWithEscalation` (when `true`).

Deserialization rules:

- `"user"` → `User`
- `"assistant"` → `Assistant(default())`
- `"assistant_with_escalation"` → `AssistantWithEscalation(default())`
- `{ model.id = "...", escalation = false }` → `Assistant(config)`
- `{ model.id = "...", escalation = true }` → `AssistantWithEscalation(config)`
- `{ model.id = "..." }` → `Assistant(config)` (`escalation` defaults to
  `false`)

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

   The user sees the sub-agent's reasoning and makes the final decision.
   If the user approves, the tool proceeds; if the user rejects, the tool fails
   with a user-rejection message distinct from a sub-agent rejection.

2. **Non-interactive mode.** Escalation follows the detached prompt policy ([RFD
   049]):

   - `deny`: fail with the sub-agent's rejection (same as `Assistant`).
   - `defaults`: use the question's default value (`true` for `apply_changes`).
   - `auto`: fail (the sub-agent already rejected the change, and auto-approving
     over that rejection would undermine the review).

#### Escalation trigger

Escalation triggers when the sub-agent's answer would cause the tool to fail.
Only boolean inquiries (such as `apply_changes`) have a general rejection value
(`false`) that the coordinator can detect before re-executing the tool.
For select and text inquiries, any valid answer proceeds, so escalation in this
RFD is defined only for boolean inquiries:

| Answer type | Escalation trigger                            |
| ----------- | --------------------------------------------- |
| Boolean     | `false` answer                                |
| Select      | Not applicable (any valid selection proceeds) |
| Text        | Not applicable (any text proceeds)            |

A future RFD could extend escalation to non-boolean questions by detecting when
a tool returns `NeedsInput` again after receiving an answer it considers
invalid.

#### Coordinator changes

The `handle_tool_result` path for `InquiryResult` has two branches: `Ok(answer)`
(re-execute tool) and `Err(error)` (fail tool).
Escalation adds a branch within `Ok`:

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

The coordinator resolves `target` for the tool/question pair with
`question_target()`, the same lookup used when routing the initial inquiry.

### Improved error message (immediate fix)

Independent of Parts 1 and 2, `modify_file.rs` replaces its static rejection
error message to drop internal jargon ("inquiry"), state the outcome clearly,
and suggest a recovery action.

Current:

```rust
"`apply_changes` inquiry was answered with `false`. Changes discarded."
```

Proposed:

```rust
"The proposed file changes were reviewed and rejected. Changes were not applied. \
 You may retry with different changes."
```

## Drawbacks

- **Reason field adds tokens.** Every inquiry response includes a 1-3 sentence
  `reason` string.
  At ~50-100 tokens per reason on a small model (Haiku), the cost is negligible
  per inquiry but adds up for tools that ask many questions.

- **Escalation adds latency on rejection.** When the sub-agent rejects an answer
  and escalation triggers, the user sees the prompt only after the sub-agent's
  round trip (~1-3 seconds on Haiku).
  That delay is preferable to a failed tool call and a spiraling main model.

- **Larger configuration surface.** `AssistantWithEscalation` adds a third
  `QuestionTarget` variant.
  The string shorthand keeps simple cases concise, but the "sub-agent with user
  fallback" behavior needs documentation.

- **Escalation is only defined for boolean inquiries.** Select and text
  questions lack a general rejection semantic, so escalation covers only boolean
  confirmations such as `apply_changes`, which are the primary use case.

## Alternatives

### Retry with feedback instead of escalation

Before escalating to the user, the coordinator could retry the inquiry with the
main model's intent as additional context (for example: "The main assistant
intended to add a TIP admonition.
The sub-agent rejected because X. Please reconsider.").

Rejected.
Retrying risks infinite loops if the sub-agent rejects again for the same
reason, adds complexity to the coordinator, and still leaves the main model
spiraling when the retry fails.
A single retry before escalation could be layered on later as an optimization.

### Coordinator intercepts `false` answers

Instead of re-executing the tool with `false`, the coordinator could synthesize
the error response directly so the tool never sees `false`.

Rejected.
This changes the tool contract.
Other answer sources (interactive user, static config) can also produce `false`,
so the tool's `apply_changes` handling is still needed.
Intercepting only sub-agent rejections in the coordinator would create a special
case that does not generalize.

### Main model explicitly requests escalation

After receiving a rejection error, the main model could call a built-in
`escalate_inquiry` tool to ask the user directly.

Deferred.
Although more general than config-based escalation, this adds a tool-call round
trip and relies on the model choosing to escalate instead of retrying or working
around the failure.
It can be explored in a future RFD as a manual override if automatic escalation
proves insufficient.

### Optional `reason` field

Make `reason` optional in the schema so the sub-agent can skip it.

Rejected.
Requiring `reason` before `answer` ensures the sub-agent performs
chain-of-thought before committing to a decision.
If `reason` is optional, smaller models frequently skip it; a required field
with a `description` hint produces reliable reasoning.

## Non-Goals

- **Escalation for select/text inquiries.** Only boolean inquiries have a clear
  rejection semantic; generalized escalation (e.g. when a tool re-asks after an
  unsatisfactory answer) is future work.
- **Batching inquiry retries.** Retrying a rejected inquiry with additional
  context before escalating is left as a future optimization.
- **User-initiated escalation override.** A mechanism for the main model to
  explicitly escalate a rejected inquiry to the user is deferred.
- **Rendering inquiry reasoning in `conversation show`.** Display formatting for
  the `reason` field in CLI output is deferred.

## Risks and Open Questions

- **Should `assistant_with_escalation` be the default for `fs_modify_file`?**
  The current default is `target = "assistant"`.
  Because a false rejection (the model spiraling or deleting files) costs far
  more than an occasional user prompt, this RFD recommends changing the project
  default to `assistant_with_escalation`.

- **Reason quality on small models.** Haiku's reasoning can be shallow (for
  example, "The change looks incorrect" without specifics).
  The schema's `description` hint helps, and per-question model overrides ([RFD
  034]) let users route important questions to more capable models.

- **Escalation UX.** When escalation triggers, the user sees a prompt for a
  question the sub-agent normally handles silently.
  Prefixing the prompt with the sub-agent's rejection ("The assistant
  recommended rejecting...") explains why the prompt appeared.

- **Interaction with RFD 018 and 049.** Both RFDs are in Discussion status.
  Escalation is a routing decision that fits into `route_prompt()` from RFD 018,
  and the detached prompt policy from RFD 049 governs escalation in
  non-interactive mode.
  Implementation must coordinate with those RFDs' timelines.

- **`auto` detached policy and escalation.** When the detached policy is `auto`
  and escalation triggers, should `auto` approve (overriding the sub-agent) or
  fail?
  This RFD proposes failing because auto-approving over a sub-agent rejection
  defeats the purpose of the review, even though `auto` then behaves differently
  for escalated questions than for direct questions.

## Implementation Plan

### Phase 0: Improve error message

Replace the static rejection error string in `modify_file.rs` with a clearer
message.
This phase has no dependencies and can be merged immediately.

### Phase 1: Reasoning in inquiry schema

Extend `create_inquiry_schema` to include the `reason` field, update the prompt
text to instruct the sub-agent to provide reasoning, and update
`LlmInquiryBackend::inquire` to return `InquiryAnswer` with both `reason` and
`answer`.
Add `reason: Option<String>` to `InquiryResponse`, and update the coordinator to
store the reason in `InquiryResponse` events and track the most recent reason
per tool for error enrichment.

Depends on Phase 0, and can be merged independently of Phase 2.

### Phase 2: Error message enrichment

When a tool fails after a sub-agent rejection, enrich the error in the
coordinator with the sub-agent's reason and model attribution so the main model
receives a structured, actionable error message.

Depends on Phase 1.

### Phase 3: Escalation target

Add `AssistantWithEscalation` to `QuestionTarget`, implement config
deserialization for the string and map forms, and add escalation logic to the
coordinator's `InquiryResult` handler.

Depends on Phase 1 (which provides the reason for the escalation prompt), and
can be merged independently of Phase 2.

### Phase 4: Default configuration

Change the project default for `fs_modify_file` from `target = "assistant"` to
`target = "assistant_with_escalation"`, and evaluate other tools with boolean
inquiries for the same change.

Depends on Phase 3.

## References

- [RFD 028: Structured Inquiry System][RFD 028]: the inquiry system this RFD
  extends.
- [RFD 034: Inquiry-Specific Assistant Configuration][RFD 034]: per-question
  model overrides and cache optimization.
- [RFD 018: Typed Prompt Routing Enum][RFD 018]: the `Prompt` enum and
  `route_prompt()` function that escalation integrates with.
- [RFD 049: Non-Interactive Mode and Detached Prompt Policy][RFD 049]: detached
  policy governing escalation in non-interactive mode.
- [RFD 005: First-Class Inquiry Events][RFD 005]: persisted inquiry events that
  gain the `reason` field.

[RFD 005]: ../005-first-class-inquiry-events.md
[RFD 018]: ../018-typed-prompt-routing-enum.md
[RFD 028]: ../028-structured-inquiry-system-for-tool-questions.md
[RFD 034]: ../034-inquiry-specific-assistant-configuration.md
[RFD 049]: ../049-non-interactive-mode-and-detached-prompt-policy.md
