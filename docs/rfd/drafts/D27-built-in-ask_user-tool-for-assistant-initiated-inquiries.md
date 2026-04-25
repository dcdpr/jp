# RFD D27: Built-in ask_user tool for assistant-initiated inquiries

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-17

## Summary

Add a built-in `ask_user` tool that lets the assistant ask the user a question
mid-turn and receive a typed answer (boolean, select, or text). This keeps the
agentic loop moving when the assistant needs human input, instead of requiring
the user to end the turn and submit a new query.

## Motivation

Today, when an assistant wants confirmation or a choice from the user — "should
I proceed?", "which of these two approaches do you prefer?", "what's the target
directory?" — there is no in-band mechanism. The turn ends with a
natural-language question in the assistant's reply, and the user has to type a
response as a new query (`jp query yes`). The agentic loop stops.

Everything needed for an in-band solution already exists:

- [RFD 028] (Implemented) defines the inquiry system that routes tool-authored
  questions through the `ToolCoordinator`, including the `Outcome::NeedsInput {
  question }` return type with `AnswerType::{Boolean, Select, Text}` and a
  `default`.
- [RFD 034] (Implemented) adds the `QuestionTarget` enum with `User` and
  `Assistant(PartialAssistantConfig)` variants, and the coordinator logic that
  prompts the user via `ToolPrompter` for `QuestionTarget::User`.
- [RFD 005] (Implemented) records `InquiryRequest`/`InquiryResponse` events in
  the persisted stream and defines `InquirySource::Assistant` — a variant that
  exists in the type but is never emitted in production code today.
- [RFD 049] (Discussion) codifies an `exclusive` flag for inquiries that
  cannot be meaningfully answered by an LLM. This RFD pre-implements a
  minimal subset of that flag so `ask_user` can signal its human-only
  contract without a name-specific escape hatch in the coordinator.

The machinery to ask the user a typed question and receive a typed answer is in
place. The missing piece is a tool surface the assistant can call. An `ask_user`
built-in tool supplies it.

## Design

### User-visible behavior

The assistant calls `ask_user` like any other tool:

```jsonc
{
  "name": "ask_user",
  "arguments": {
    "question": "The current approach modifies production config in place. Apply with backup, apply without backup, or abort?",
    "answer_type": "select",
    "options": ["backup", "overwrite", "abort"],
  },
}
```

The user sees the question as an inline prompt — the same UI used today for
tool-authored questions. They answer; the answer becomes the tool's result;
the agentic loop continues.

### Arguments

```jsonc
{
  "question": "string",                     // required
  "answer_type": "boolean"|"select"|"text", // default: "text"
  "options": ["string", ...],               // required when answer_type == "select"
  "default": <any>                          // optional — pre-populates the prompt
}
```

These map directly onto `jp_tool::AnswerType` and `jp_tool::Question`. Invalid
combinations (e.g. `select` without options) return `Outcome::Error` so the LLM
learns to correct the call.

### Execution flow

`ask_user` is a `BuiltinTool`:

- First call: `answers` is empty. The tool validates arguments and returns
  `Outcome::NeedsInput { question }`.
- Second call: `answers` contains the response keyed by the question ID. The
  tool returns `Outcome::Success { content: <answer as text> }`.

The existing coordinator path handles everything in between: `ToolPrompter`
shows a styled prompt (boolean gets the git-style `y/Y/n/N`, select gets an
inline select, text gets a text input), the answer is stored in the turn, the
tool is re-spawned with the accumulated answer, and the result becomes a normal
`ToolCallResponse`.

### Tool configuration

Built-in config registered in `jp_cli::cmd::query::tool::builtins::all()`:

- `enable: Enable::Sticky` — a new variant introduced by this RFD (see
  [Enable variant for bare-disable immunity](#enable-variant-for-bare-disable-immunity)).
  Enabled by default; a bare `-T` (disable all) skips it; a named `-T
  ask_user` or `conversation.tools.ask_user.enable = false` disables it.
  Contrast with `Enable::Always` (`describe_tools`), which also refuses named
  disables.
- `run: Unattended`, `result: Unattended` — the question itself is the user
  interaction; no permission or delivery prompts.
- `style.hidden: true` — the tool call and result are not rendered as tool
  chrome. The user only sees the prompt and their own answer flowing with the
  assistant's reply.

The tool's `Question` is authored in Rust with `target: User` and `exclusive:
true` (see [Non-interactive behavior](#non-interactive-behavior)). The
`exclusive` default comes from the tool's `BuiltinTool` implementation, not
from user config.

### Enable variant for bare-disable immunity

The existing `Enable` variants cannot express "enabled by default; immune to
bare `-T` (disable all); disableable via `-T ask_user` (disable named)":

| Variant           | `-T` (bare)    | `-T ask_user` (named) |
|-------------------|----------------|-----------------------|
| `On`              | disables       | disables              |
| `Explicit`        | disables       | disables              |
| `Always`          | skipped        | **errors**            |
| **New: `Sticky`** | **skipped**    | **disables**          |

`Sticky` is the disable-side mirror of `Explicit`: where `Explicit` says
"requires a named directive to enable," `Sticky` says "requires a named
directive to disable."

Required changes:

1. Add `Enable::Sticky` to `jp_config::conversation::tool::Enable` with the
   usual companions: `is_sticky()` helper, `FromStr` accepting `"sticky"`,
   `Serialize` emitting `"sticky"`, `Display`, and the `schematic` schema
   entry.
2. In `jp_cli::cmd::query::apply_enable_tools`:
   - `ToolDirective::DisableAll` skips `Sticky` (same filter as `Always`).
   - `ToolDirective::Disable(name)` applies to `Sticky` (unlike `Always`,
     which errors via the "system tool cannot be disabled" guard).

`Sticky`'s interaction with [RFD 055]'s group-level `-T GROUP` is an open
question for that RFD; the author leans toward "groups disable `Sticky`"
because groups are named directives, not bare, but defers the final call to
[RFD 055].

### Event recording

User-answered tool questions go through `ToolPrompter` today and do **not** emit
`InquiryRequest`/`InquiryResponse` events ([RFD 005] is explicit on this). For
`ask_user`, those events are the archival value of the feature — without them,
the persisted stream only shows `ToolCallRequest(ask_user) →
ToolCallResponse(<answer>)` and loses the semantic signal that the assistant
asked the user something.

The coordinator is extended to record the `InquiryRequest`/`InquiryResponse`
pair around a user-answered question when the inquiry's source is
`InquirySource::Assistant`. For `ask_user` specifically, the source is
`Assistant`, not `Tool { name: "ask_user" }`. `InquirySource::Assistant` already
exists in the type definitions — this RFD is its first production use site.

The behavior is scoped to `InquirySource::Assistant`. User-answered questions
from ordinary tools continue to behave as they do today. A small amendment to
[RFD 005]'s "Recording Inquiry Events" section captures the new case.

### Non-interactive behavior

When no TTY is available, the `ToolCoordinator` currently routes
`QuestionTarget::User` questions through the `InquiryBackend` — i.e. it asks the
LLM to answer its own question. For `ask_user`, this is nonsensical: the
assistant called the tool *because* it wants human input; auto-resolving with a
sub-agent defeats the purpose.

**This RFD does not introduce a new `QuestionTarget` variant.** [RFD 019]'s
proposal of `QuestionTarget::UserOnly` was rejected because "exclusivity is
orthogonal to target — it describes whether the target can be overridden when
unavailable, not who the target is." [RFD 049] (Discussion) codifies the
replacement: an `exclusive` flag on the question plus a detached-policy cascade
that decides what happens when no client is attached.

**This RFD pre-implements a minimal subset of [RFD 049] Phase 1**: the
`exclusive: bool` field on `jp_tool::Question` (default `false`), plus the
coordinator routing that consumes it. When a user-targeted question has
`exclusive == true` and no TTY is available, the coordinator fails the tool
with a clear error (`"<tool_name> requires an interactive terminal"`,
non-transient) instead of routing through the `InquiryBackend`.

`ask_user` authors its question with `exclusive: true`. No other built-in or
user tool sets the flag today, so non-TTY behavior for all other tools is
unchanged.

Scope of the pre-implementation vs. [RFD 049] Phase 1 in full:

| [RFD 049] Phase 1 scope                                      | In this RFD |
|--------------------------------------------------------------|:-----------:|
| `exclusive: bool` on `jp_tool::Question` (tool default)      |      ✓      |
| Coordinator consumes `exclusive` for fail-vs-inquire routing |      ✓      |
| `exclusive` on `QuestionConfig` (user override)              |      ✗      |
| `DetachedMode` enum and detached-policy cascade              |      ✗      |
| `--non-interactive` CLI flag                                 |      ✗      |

When [RFD 049] lands in full, it strictly extends this work: the user-override
layer merges on top of the tool-level default, and the `DetachedMode` cascade
decides what an `auto`/`defaults`/`deny` policy does with an `exclusive`
question. `ask_user` needs no code changes at that point.

## Drawbacks

- **One more built-in tool to maintain.** The tool itself is small (~50 LOC plus
  the builtin config entry) but adds a surface the assistant can call, with new
  failure modes to test.
- **Unclear pedagogical signal for the LLM.** Providers sometimes over-use new
  tools. `ask_user` should only fire when the assistant genuinely needs input
  the conversation doesn't supply. The tool description must discourage
  reflexive use ("When in doubt, answer the user's original question; do not
  call `ask_user` for clarification you can derive from context.").
- **Two `Assistant`-sourced paths to keep aligned.** If a future RFD introduces
  another assistant-originated inquiry path (for example a structured-output
  driven assistant-to-user prompt that bypasses tool calls), both must agree on
  the same source semantics.
- **New `Enable` variant adds enum surface area.** `Sticky` is the fifth
  variant on an already-nuanced enum. Every new variant is another branch the
  `apply_enable_tools` logic and any future `Enable`-aware code has to
  consider. The justification is the lack of a close-enough existing variant;
  the counterfactual — name-specific coordinator logic — is worse.

## Alternatives

### Assistant-emitted inquiry directive (no tool)

Have the assistant emit an inquiry through a structured-output schema or a
special control token, with the turn loop pausing on detection. Rejected:
provider-specific (structured output support is uneven), mixing structured
output with tool-call streaming is fragile, and the free agentic-loop
continuation offered by tool calls disappears.

### Add `QuestionTarget::UserOnly`

Rejected, consistent with [RFD 019]'s own rejection of this variant. Exclusivity
belongs on `Question` (per [RFD 049]), not as a routing target.

### Name-specific guard in the coordinator

Short-circuit `InquiryBackend` routing when the tool's name is `ask_user`.
Rejected: adds a named exception to the coordinator that has to be torn out
again when [RFD 049]'s `exclusive` lands, and creates drift between `ask_user`
and any future tool that wants the same human-only contract.

### Use `InquirySource::Tool { name: "ask_user" }` instead of `Assistant`

The existing flow uses `Tool { name: <tool> }` for tool-authored questions.
Applying it here keeps the coordinator uniform but throws away the semantic
distinction `InquirySource::Assistant` was defined for. Rejected: a
conversation-level reader (`jp conversation show`, future UI) benefits from
knowing the assistant originated the inquiry, independent of the tool name used
as transport.

### Skip event recording

Record only the `ToolCallRequest`/`ToolCallResponse` pair, as happens today for
user-answered tool questions. Rejected: the archival value of assistant-to-user
exchanges is the semantic signal. Leaving it out reduces `ask_user` to sugar
over a generic tool and makes future UI work harder.

### Use `Enable::Always` for the tool

Give `ask_user` the same `Always` semantic as `describe_tools`. Rejected:
`Always` refuses named `-T ask_user` via the explicit guard in
`apply_enable_tools`. The user must be able to disable `ask_user` by name
without tripping an error.

### Change `Enable::Always` semantics instead of adding `Sticky`

Rather than a new variant, loosen `Always` so named disables succeed and only
bare `-T` is refused. Rejected: `describe_tools` relies on the strict
semantic, and loosening it invites users to break tool discovery by accident
(`-T describe_tools` would disable the tool the LLM uses to learn about
others). Keep `Always` strict; add `Sticky` as the weaker variant.

## Non-Goals

- **Assistant-to-assistant escalation or reasoning trails.** This RFD does not
  address sub-agent reasoning or escalation from assistant to user on rejection.
  Those are orthogonal concerns.
- **Full [RFD 049] Phase 1 scope.** This RFD implements only
  `jp_tool::Question.exclusive` and the single coordinator consumer needed
  for `ask_user`. The `QuestionConfig` user override, the `DetachedMode`
  cascade, and the `--non-interactive` CLI flag remain [RFD 049]'s
  responsibility.
- **Rendering of `InquiryRequest`/`InquiryResponse` with
  `InquirySource::Assistant` in `jp conversation show`.** Display formatting is
  deferred, consistent with [RFD 005]'s own Non-Goals.
- **Structured-output answers.** `answer_type` is limited to `boolean`,
  `select`, and `text` — the set already supported by `ToolPrompter`. Structured
  answers are out of scope.

## Risks and Open Questions

- **Misuse by the assistant.** If the LLM calls `ask_user` frequently and
  inappropriately, the experience degrades into a chatbot asking permission
  for everything. The tool description and built-in config defaults must
  discourage reflexive use. Real-world usage should be monitored during
  rollout.
- **Interaction with [RFD 049]'s detached policy.** This RFD pre-implements a
  subset of [RFD 049] Phase 1 (`Question.exclusive` + the single coordinator
  consumer). When [RFD 049] lands in full, the `DetachedMode` cascade decides
  what `auto`/`defaults`/`deny` do with an `exclusive` question. `ask_user`
  inherits that behavior with no code change — the `exclusive: true` default
  on its question is already in place.
- **Scope creep into [RFD 049].** By pre-implementing `exclusive`, this RFD
  takes on work that [RFD 049] owns. The risk is coordinating changes if
  [RFD 049] evolves the field's shape (type, default, semantics) before
  merging. Mitigation: keep the scope as narrow as possible — one field, one
  routing check, no user-facing config — so any [RFD 049] divergence is a
  small fix.
- **Interaction with [RFD 018]'s `Prompt` enum.** [RFD 018] is in Discussion.
  Its `Prompt::ToolQuestion` variant will carry `ask_user`'s question with no
  special casing. No [RFD 018] blocker for this work.
- **[RFD 005] amendment scope.** [RFD 005] states user-answered questions do
  not produce inquiry events. This RFD records events when `source ==
  Assistant`. The amendment is small but should be explicit when this RFD
  lands.
- **Interaction with [RFD 055]'s `Enable` changes.** [RFD 055] extends
  `Enable` with `explicit_or_group` on the enable-direction side. This RFD
  adds `Sticky` on the disable-direction side. The two additions are
  orthogonal and should compose without conflict, but the final
  interaction table in [RFD 055] needs a row for `Sticky` (proposed:
  `--tools GROUP` enables, `--no-tools GROUP` disables — groups are named
  directives, not bare).

## Implementation Plan

### Phase 1: `Enable::Sticky` variant

Add `Enable::Sticky` to `jp_config::conversation::tool::Enable` with the
usual companions (`is_sticky`, `FromStr`, `Serialize`, `Display`,
`schematic`). In `jp_cli::cmd::query::apply_enable_tools`:

- `ToolDirective::DisableAll` skips `Sticky` (same filter as `Always`).
- `ToolDirective::Disable(name)` applies to `Sticky` (unlike `Always`, which
  errors).

Unit tests for the four variant interactions against bare and named
directives.

Can be merged independently of Phase 2.

### Phase 2: Built-in tool

Add `jp_llm::tool::builtin::ask_user::AskUser` implementing `BuiltinTool`.
Add the `ask_user()` config entry to
`jp_cli::cmd::query::tool::builtins::all()` with `enable: Enable::Sticky`.
Register the builtin in `handle_turn`. Unit tests for argument validation
and the `NeedsInput → Success` round-trip.

Depends on Phase 1. No coordinator changes. Without Phase 3 the
non-interactive behavior is "route to inquiry backend" — wrong for
`ask_user`.

### Phase 3: `exclusive` field and coordinator routing

Add `exclusive: bool` to `jp_tool::Question` (default `false`, serde skip
when default). In `ToolCoordinator::handle_tool_result`, when about to route
a user-targeted question to the inquiry backend because of no TTY, check
`question.exclusive` first. If set, synthesize a tool-level error response
(`"<tool_name> requires an interactive terminal"`, non-transient) instead of
routing.

This is a subset of [RFD 049] Phase 1. When [RFD 049] lands in full, the
`QuestionConfig` override merges on top of the tool-level default; no
coordinator change is needed here.

Depends on Phase 2. Can be merged independently of Phase 4.

### Phase 4: Event recording for assistant-originated inquiries

Extend the coordinator to emit `InquiryRequest` with `InquirySource::Assistant`
and the matching `InquiryResponse` when the user answers a question asked by
`ask_user`. Amend [RFD 005] to describe the new recording case.

Depends on Phase 2.

## References

- [RFD 005] — defines `InquirySource` and inquiry event recording rules.
- [RFD 028] — the inquiry coordinator this RFD reuses.
- [RFD 034] — defines the current `QuestionTarget` shape.
- [RFD 018] — future `Prompt` enum that will carry this tool's question
  without special casing.
- [RFD 049] — defines the full `exclusive` flag and detached-policy cascade
  that this RFD pre-implements a subset of.
- [RFD 055] — tool groups and the broader `Enable` variant restructuring; this
  RFD adds `Sticky` as a disable-side mirror of `Explicit`, orthogonal to
  [RFD 055]'s `explicit_or_group`.
- [RFD 019] — abandoned; referenced for the original `QuestionTarget::UserOnly`
  rejection rationale.

[RFD 005]: 005-first-class-inquiry-events.md
[RFD 018]: 018-typed-prompt-routing-enum.md
[RFD 019]: 019-non-interactive-mode.md
[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 034]: 034-inquiry-specific-assistant-configuration.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 055]: 055-tool-groups.md
