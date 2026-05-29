# RFD 083: Built-in ask\_user tool for assistant-initiated inquiries

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-17
- **Requires**: [RFD 028], [RFD 034], [RFD 081], [RFD 082]

## Summary

Add a built-in `ask_user` tool that lets the assistant ask the user a question
mid-turn and receive a typed answer (boolean, select, or text).
This keeps the agentic loop moving when the assistant needs human input, instead
of requiring the user to end the turn and submit a new query.

## Motivation

Today, when an assistant wants confirmation or a choice from the user — "should
I proceed?", "which of these two approaches do you prefer?", "what's the target
directory?" — there is no in-band mechanism.
The turn ends with a natural-language question in the assistant's reply, and the
user has to type a response as a new query (`jp query yes`).
The agentic loop stops.

The transport machinery for an in-band solution already exists:

- [RFD 028] (Implemented) defines the inquiry system that routes tool-authored
  questions through the `ToolCoordinator`, including the `Outcome::NeedsInput {
  question }` return type with `AnswerType::{Boolean, Select, Text}` and a
  `default`.
- [RFD 034] (Implemented) adds the `QuestionTarget` enum with `User` and
  `Assistant(PartialAssistantConfig)` variants, and the coordinator logic that
  prompts the user via `ToolPrompter` for `QuestionTarget::User`.
- [RFD 049] (Discussion) proposes an `exclusive` flag for inquiries that cannot
  be meaningfully answered by an LLM.
  This RFD pre-implements a minimal subset of that flag so `ask_user` can refuse
  the LLM-fallback route without a name-specific escape hatch in the
  coordinator.
- [RFD 081] (Discussion) decomposes the tool `enable` field into `{ state,
  allow_toggle }`, which gives `ask_user` a principled way to be enabled by
  default while remaining disableable by name.
  This RFD adopts that shape directly.

The wire transport (LLM emits a tool call, JP routes it, the user is prompted)
is in place.
The missing piece is the tool surface the assistant can call.
An `ask_user` built-in tool supplies it.

Along the way, this RFD adds two small generic enrichments to tool questions
that `ask_user` needs and other tools can opt into: a `persistence` policy
(opt-out of "remember for turn" for high-risk confirmations) and an `exclusive`
flag (refuse the LLM-fallback route when no TTY is available).
A display-only `prompt_label` field on `QuestionConfig` lets the prompter render
a "who's asking" header — `ask_user` uses it to render `"Assistant"`. 083 also
widens the persisted `InquiryQuestion` shape with the same fields so 082's
recording infrastructure surfaces the new metadata in the conversation stream.
Unified recording itself — the lifecycle, the `Cancelled` variant, the
source-attribution hook — is [RFD 082]'s responsibility; 083 plugs into it.

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
tool-authored questions, attributed to the assistant rather than to a tool.
They answer; the answer becomes the tool's result; the agentic loop continues.

### Arguments

```jsonc
{
  "question": "string",                     // required, single line
  "context": "string",                      // optional — context shown above the question
  "answer_type": "boolean"|"select"|"text", // default: "text"
  "options": ["string", ...],               // required when answer_type == "select"
  "default": <any>                          // optional — pre-populates the prompt
}
```

These map directly onto `jp_tool::AnswerType` and `jp_tool::Question`.
The tool validates arguments and returns `Outcome::Error` (with an actionable
message the LLM can learn from) for any of:

- `question` missing or empty.
- `question` contains a newline.
  The question text is required to be a single line; any multi-line context
  belongs in `context`.
- `answer_type == "select"` with `options` missing or empty.
- `options` set when `answer_type != "select"`.
- `default` of the wrong type for the selected `answer_type` (e.g. a string
  default with `answer_type: "boolean"`).
- `default` not present in `options` when `answer_type == "select"`.

On the second call, the tool also validates the accumulated answer against the
resolved `answer_type` and `options` before returning `Outcome::Success`.
The same `Outcome::Error` path applies for:

- Answer JSON shape mismatched against `answer_type` (defensive against any
  routing path that produces a mistyped value).
- `answer_type == "select"` and the answer is not present in `options`.

The executor's error wording is generic ("the accumulated answer does not match
the requested answer type").
The source-aware error for the common cause — a `QuestionConfig.answer`
configured with a type-incompatible value, or absent from `options` — is
produced earlier by the coordinator's static-answer short-circuit; it names
`QuestionConfig.answer` as the source so the LLM does not retry a
model-blameless config error.
See [Implementation Plan](#phase-1-generic-enrichments) for the validation step.

`context` is rendered above the question line, matching the existing
`ToolPrompter` behavior for `Question.context` (renamed from `pre_amble` in this
RFD, see [Implementation Plan](#phase-1-generic-enrichments)).
Using a separate argument keeps the question single-line by construction — no
string-splitting heuristic that could reorder context and question.

### Execution flow

`ask_user` is a `BuiltinTool`:

- First call: `answers` is empty.
  The tool validates arguments and returns `Outcome::NeedsInput { question }`.
  The authored `Question` carries `exclusive: true` and `persistence:
  AnswerReusePolicy::None` — see [Generic enrichments to tool
  questions](#generic-enrichments-to-tool-questions).
  The prompt's "who's asking" label is `"Assistant"`, set by the tool's
  registered `QuestionConfig.prompt_label` (see [Tool configuration]
  (\#tool-configuration)).
- Second call: `answers` contains the response keyed by the question ID.
  The tool returns `Outcome::Success { content: <JSON-encoded answer> }`.

The built-in always emits a single question with `id: "answer"`.
This is the slot referenced by `conversation.tools.ask_user.questions.answer.*`
in user config (see [Tool configuration](#tool-configuration)), the key under
which the answer is stored in the turn, and the lookup target for the registered
`prompt_label`.

The success body is a stable JSON shape rather than the raw answer text, so the
LLM receives type information alongside the value:

```json
{
  "answer_type": "boolean",
  "answer": true
}
{
  "answer_type": "select",
  "answer": "backup"
}
{
  "answer_type": "text",
  "answer": "/tmp/output"
}
```

This preserves the typed contract end-to-end: `true` (boolean) is
distinguishable from `"true"` (select option) and `"true"` (text answer),
regardless of how the provider stringifies the tool response.

The coordinator handles everything in between: `ToolPrompter` renders the
question (honoring the configured `prompt_label` for the "who's asking" header,
honoring `persistence` for the `Y`/`N` "remember for turn" affordance), the
answer is stored in the turn, the tool is re-spawned with the accumulated
answer, and the result becomes a normal `ToolCallResponse`.

[RFD 082] owns the baseline recording lifecycle: every `Outcome::NeedsInput`
round-trip produces an `InquiryRequest`/ `InquiryResponse` pair, regardless of
routing path. 083 contributes on top of that: a `BuiltinTool::inquiry_source()`
override for `ask_user`, the `InquiryQuestion` widening for
`context`/`exclusive`/`persistence`, a new
`CancellationReason::InvalidStaticAnswer` variant for the static-answer
validation it introduces, and the emit sites for 083's routing paths (which
reuse 082's `NoPromptBackend` and `AssistantRoutingDenied` variants).
See [Persisted recording] (\#persisted-recording) below for the full breakdown.

### Tool configuration

Built-in config registered in `jp_cli::cmd::query::tool::builtins::all()`,
modeled after the existing `describe_tools` entry:

- `source: ToolSource::Builtin { tool: None }`.

- `enable: Enable { state: true, allow_toggle: IfNamed }` — enabled by default,
  immune to bare `-T` (disable-all), disableable by name.
  CLI: `-T ask_user`.
  TOML: `conversation.tools.ask_user.enable = { state = false }` — disables
  while preserving the `if_named` toggle policy via RFD 081's partial-merge
  rules.
  Note that the bool shorthand `enable = false` also disables but resets
  `allow_toggle` to `always`, which may not be the intent.
  This is the shape [RFD 081] introduces; 083 consumes it without contributing
  any new variant or predicate of its own.

- `run: Unattended`, `result: Unattended` — the question itself is the user
  interaction; no permission or delivery prompts.

- `style.hidden: true` — the tool call and result are not rendered as tool
  chrome.
  The user only sees the prompt and their own answer flowing with the
  assistant's reply.

- `questions.answer.prompt_label = "Assistant"` — the question-config default
  that makes the prompter render `"Assistant"` as the "who's asking" header.
  See [Generic enrichments](#generic-enrichments-to-tool-questions) below.
  This is a display-only field; provenance for the persisted stream is a
  separate concern handled by [RFD 082].

- `description`: a long-form description that establishes when to call
  `ask_user` and discourages reflexive use.
  Suggested wording:
  
  > Ask the user a typed question (boolean, select, or text) and receive their
  > answer.
  > Use only when the conversation does not provide the information you need and
  > the user can reasonably be expected to answer.
  > When in doubt, answer the user's original question directly; do not call
  > `ask_user` for clarification you can derive from context or for confirmation
  > of obvious next steps.
  > Do not use `ask_user` to collect secrets (passwords, API keys, SSH
  > passphrases): answers are returned to the model and persisted in the
  > conversation stream.
  
  This long-form text is set as `description` rather than `summary` so that
  `ToolDocs::schema_description()` (which prefers `summary` and falls back to
  `description`) sends it to the provider in every request — the usage guard
  must be model-visible to be effective.
  No separate `summary` is set.

- `parameters`: explicit JSON Schema for each argument the LLM may pass.
  The schema is provider-valid (no `"type": "any"`, no static
  conditional-requiredness); cross-field constraints are enforced at runtime in
  `AskUser::execute` and surface as `Outcome::Error` to the LLM (see
  [Arguments](#arguments) above).
  
  ## | Parameter | Type emitted | Required | Notes | | ------------- | ----------------------- | -------- |
  
  | | `question` | `string` | yes | The question text.
  Must be single-line; runtime rejects newlines.
  | | `context` | `string` | no | Optional multi-line context rendered above the
  question.
  | | `answer_type` | `string` | no | Emitted with `"enum": ["boolean",
  "select", "text"]`.
  Defaults to `"text"`.
  | | `options` | `array` of `string` | no | At runtime, required when
  `answer_type == "select"` and forbidden otherwise.
  | | `default` | `["boolean", "string"]` | no | Multi-type to cover boolean
  defaults and string defaults (`select` / `text`).

The tool's `Question` is authored in Rust with `exclusive: true` and
`persistence: AnswerReusePolicy::None`.
Those tool-author defaults come from the `BuiltinTool` implementation, not from
user config (consistent with [RFD 049]'s pattern for `exclusive`).

#### Supported `questions.answer` configuration

`ask_user` reuses the existing `QuestionConfig` shape, so every field that shape
exposes is technically settable.
Not every combination is meaningful for this tool:

| Field                  | Supported for `ask_user`? | Notes                                                                                                                                                                                               |
| ---------------------- | ------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `answer`               | yes                       | Fixed answer; the prompt is never shown. Reasonable for tests or automation.                                                                                                                        |
| `target = "user"`      | yes                       | Default. The prompt is shown to the human.                                                                                                                                                          |
| `target = "assistant"` | **rejected at routing**   | Rejected by `exclusive`: see [Non-interactive behavior](#non-interactive-behavior). `ask_user` authors `exclusive: true`, which blocks both the no-TTY fallback *and* explicit assistant-targeting. |
| `prompt_label`         | yes                       | Display-only header. `ask_user`'s default is `"Assistant"`. Users may override but rarely should.                                                                                                   |

The rejection of `target = "assistant"` is generic on both sides: the routing
check is `if question.exclusive && resolved_target == Assistant: fail`.
No tool-name branching in coordinator code; the constraint flows from
`ask_user`'s tool-authored `exclusive: true`.

#### Built-in aliasing

Users cannot meaningfully alias `ask_user` under a different config key (e.g.
`source = "builtin.ask_user"` on a tool named `ask`): the built-in executor
lookup uses the config key, not `ToolSource::Builtin { tool }`, and fails with
`ToolError::NotFound` at execution time.
This is unchanged by this RFD; it affects every built-in.
A future RFD can address aliasing uniformly across built-ins.

### Generic enrichments to tool questions

083 makes two small additions to `jp_tool::Question` and one to
`QuestionConfig`, all designed generically and applying uniformly to every tool
that emits questions:

```rust
// jp_tool::Question (in-flight question shape)
pub struct Question {
    // existing fields: id, text, context (renamed from pre_amble), answer_type, default
    // …

    /// Whether the question can be meaningfully answered by anything other
    /// than a human. Pre-implements [RFD 049]'s `exclusive` subset. When
    /// `true`, the coordinator refuses both the no-TTY fallback route
    /// (via the inquiry backend) and explicit `target = "assistant"`
    /// routing.
    pub exclusive: bool,

    /// How an answer may be persisted within the turn.
    pub persistence: AnswerReusePolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerReusePolicy {
    /// Re-ask every time. No "remember for turn" affordance offered.
    None,

    /// Today's behavior: booleans show the git-style `y`/`Y`/`n`/`N`
    /// options, where the uppercase variants remember the answer for the
    /// rest of the turn. The default for backwards compatibility.
    #[default]
    Turn,
}

// Serializes as "none" / "turn" via `rename_all = "snake_case"`. The
// `Default` impl returning `Turn` lets `#[serde(default)]` on the
// `Question.persistence` field deserialize absent input as `Turn`.
```

```rust
// jp_config::conversation::tool::QuestionConfig (per-question config)
pub struct QuestionConfig {
    // existing fields: target, answer
    // …

    /// Optional "who's asking" header rendered above the question text.
    /// Display-only — has no effect on routing or on the persisted
    /// `InquirySource`. `None` (the default) preserves today's visual
    /// (no extra header). `ask_user`'s registered config sets this to
    /// `Some("Assistant")`.
    pub prompt_label: Option<String>,
}
```

Defaults (`AnswerReusePolicy::Turn`, `exclusive: false`, `prompt_label: None`)
preserve today's behavior.
Existing tool-questions inherit the defaults and see no change; tools that opt
in declare their preference either at runtime (`exclusive`, `persistence` on the
in-flight `Question`) or at config time (`prompt_label` in their builtin
`PartialToolConfig` entry, the same way existing tools declare `target` or fixed
`answer`).

The single rendering path — `ToolPrompter::prompt_question` — receives the
in-flight question alongside the pre-resolved `prompt_label` from the
coordinator, and honors the question's `persistence` policy.
The coordinator owns the lookup: it reads
`QuestionConfig::prompt_label.as_deref()` and passes it through; the prompter
renders the header when present and skips it when `None` (preserving today's
visual for existing tool-questions).

`prompt_label` is purely cosmetic.
It does **not** influence the persisted `InquirySource` — source derivation
lives in [RFD 082] and reads from tool metadata, not from user-overridable
config.
The two are deliberately separate concerns: a user can relabel the header
without lying in the audit record, and tool metadata can declare source without
dictating UI.

#### Propagating to the persisted shape

[RFD 082] introduces the recording infrastructure against the existing
`InquiryQuestion` shape (which already lives in
`jp_conversation::event::inquiry`, introduced by [RFD 005]). 083 widens that
persisted type with the same source-side fields, so the recording site can
capture the prompt context the user saw and the policy that drove routing:

```rust
// jp_conversation::event::inquiry::InquiryQuestion
pub struct InquiryQuestion {
    // existing: text, answer_type, default
    // …

    /// Optional multi-line context rendered above the question.
    /// Newly added in this RFD; matches the rename of
    /// `jp_tool::Question.pre_amble` to `Question.context`.
    pub context: Option<String>,

    /// Whether the question was marked human-only at the source.
    pub exclusive: bool,

    /// How an answer may be persisted within the turn.
    pub persistence: InquiryAnswerReusePolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InquiryAnswerReusePolicy {
    None,
    #[default]
    Turn,
}
```

`InquiryAnswerReusePolicy` lives in `jp_conversation::event::inquiry` so the
persisted-event crate does not depend on the in-flight `jp_tool` crate.
The coordinator's recording site converts from `jp_tool::AnswerReusePolicy` to
`jp_conversation::InquiryAnswerReusePolicy` at the boundary.

Field defaults preserve today's serialization shape.
`persistence` carries `#[serde(default, skip_serializing_if =
"InquiryAnswerReusePolicy::is_turn")]` so legacy events without the field
deserialize as `Turn`, and today's default serializes to nothing extra on the
wire.
`exclusive` carries `#[serde(default, skip_serializing_if = "is_false")]`.
`context` defaults to `None` and skips serialization when absent.

### Persisted recording

[RFD 082] supplies the recording infrastructure 083 builds on: the unified
recording lifecycle (every `Outcome::NeedsInput` round-trip produces an
`InquiryRequest`/`InquiryResponse` pair), the `InquiryResponse::Cancelled`
variant with `User`, `BackendError`, `NoPromptBackend`, and
`AssistantRoutingDenied` reasons (the last two introduced for 082's own
`AnswerType::Secret` routing guard), the turn-cache split, and the
`BuiltinTool::inquiry_source()` hook that defaults to `InquirySource::Tool {
name }`. 083 reuses 082's `NoPromptBackend` and `AssistantRoutingDenied` for its
`exclusive: true` routing fail-fast paths — the guard machinery is shared. 083
contributes only one new variant of its own (`InvalidStaticAnswer`) and the emit
sites for the routing paths and the static-answer validation.

083 plugs into 082's recording infrastructure and contributes:

- The `BuiltinTool::inquiry_source()` override on `AskUser` returns
  `InquirySource::Assistant`, so `ask_user`'s exchanges persist with assistant
  provenance.

- The `InquiryQuestion` widening propagates `context`, `exclusive`, and
  `persistence` to the persisted shape (see [Propagating to the persisted
  shape](#propagating-to-the-persisted-shape) above) so the recording site
  captures the prompt context the user saw and the policy that drove routing.

- One new `CancellationReason` variant: `InvalidStaticAnswer` (a configured
  `QuestionConfig.answer` that did not match the in-flight question's
  `answer_type` or `options`). 082 documents its `CancellationReason` enum as
  open to extension; 083 adds this variant in the same module.

- The emit sites for 083's routing paths and static-answer validation, reusing
  082's variants where appropriate:
  
  | Condition | Persisted reason | |
  ------------------------------------------------------ |
  ------------------------ | | `target = "user"`, no TTY, `exclusive = true` |
  `NoPromptBackend` | | `target = "assistant"`, `exclusive = true` |
  `AssistantRoutingDenied` | | `QuestionConfig.answer` invalid for the question
  shape | `InvalidStaticAnswer` | |

083 cannot ship before 082.
The hook 083 overrides, the `Cancelled` variant, the recording lifecycle, and
the `NoPromptBackend` / `AssistantRoutingDenied` reasons all originate in 082;
083 introduces fields, routing paths, the static-answer validation, and the
`InvalidStaticAnswer` reason on top.

### Non-interactive behavior

When no TTY is available — meaning the current `is_tty` heuristic
(`io::stdout().is_terminal()`) returns `false` — the `ToolCoordinator`
currently routes `QuestionTarget::User` questions through the `InquiryBackend`,
i.e. it asks the LLM to answer its own question.
For `ask_user`, this is nonsensical: the assistant called the tool *because* it
wants human input; auto-resolving with a sub-agent defeats the purpose.
([RFD 049] may later replace the `is_tty` heuristic with `/dev/tty` detection;
`ask_user` inherits the new behavior with no code change.)

**This RFD does not introduce a new `QuestionTarget` variant.** [RFD 019]'s
proposal of `QuestionTarget::UserOnly` was rejected because "exclusivity is
orthogonal to target — it describes whether the target can be overridden when
unavailable, not who the target is."
[RFD 049] (Discussion) proposes the replacement: an `exclusive` flag on the
question plus a detached-policy cascade that decides what happens when no client
is attached.

**This RFD pre-implements a minimal subset of [RFD 049] Phase 1**: the
`exclusive: bool` field on `jp_tool::Question` (default `false`, added as part
of [Generic enrichments](#generic-enrichments-to-tool-questions)), plus the
coordinator routing that consumes it.

`exclusive: true` means "this question can only be answered by a human."
The coordinator enforces this by refusing two routing paths:

1. **No-TTY fallback to the inquiry backend.** When a user-targeted question has
   `exclusive == true` and no TTY is available, the coordinator fails the tool
   with a clear error that tells the LLM not to retry this turn (`"<tool_name>
   cannot run because no interactive terminal is available. Do not retry this
   tool call in this turn; continue without user input or explain what
   information is missing."`) instead of routing through the `InquiryBackend`.
2. **Explicit `target = "assistant"`.** When a question has `exclusive == true`
   and the resolved `QuestionConfig.target` is `Assistant`, the coordinator
   fails the tool with a similar error before routing.
   This is a generic check (`if question.exclusive && resolved_target ==
   Assistant: fail`) and applies to any tool that opts into `exclusive: true`,
   not only to `ask_user`.
   The check eliminates the need for tool-name-specific guards in the
   coordinator.

`exclusive` does **not** block a configured `QuestionConfig.answer`: a user who
pinned a fixed answer in their config has opted out of routing altogether, and
the static-answer short-circuit applies regardless of routing policy.

`ask_user` authors its question with `exclusive: true`.
No other built-in or user tool sets the flag today, so behavior for all other
tools is unchanged.
When [RFD 049] lands the per-question user override, a user who deliberately
wants `target = "assistant"` on a tool that ships `exclusive: true` can set
`exclusive = false` in their config to opt out of the human-only contract.

**Resolution precedence for tool questions:**

| Configuration                                  | Routing outcome                                      |
| ---------------------------------------------- | ---------------------------------------------------- |
| `QuestionConfig.answer` set                    | Use the configured answer. Routing skipped entirely. |
| `target = "user"`, TTY available               | Prompt the user (existing behavior).                 |
| `target = "user"`, no TTY, `exclusive = false` | Route to inquiry backend (existing behavior).        |
| `target = "user"`, no TTY, `exclusive = true`  | **Fail the tool** (new in this RFD).                 |
| `target = "assistant"`, `exclusive = false`    | Route to inquiry backend (existing behavior).        |
| `target = "assistant"`, `exclusive = true`     | **Fail the tool** (new in this RFD).                 |

Future work in [RFD 049] adds a `QuestionConfig.exclusive` user-override layer
(so a user can soften a tool's `exclusive: true` to `false`) and a
`DetachedMode` cascade for finer-grained no-TTY behavior.
Those layers merge over the rows above without invalidating them.

Scope of the pre-implementation vs. [RFD 049] Phase 1 in full:

| [RFD 049] Phase 1 scope                                  | In this RFD |
| ------------------------------------------------------------ | :---------: |
| `exclusive: bool` on `jp_tool::Question` (tool default)      |      ✓      |
| Coordinator consumes `exclusive` for fail-vs-inquire routing |      ✓      |
| `exclusive` on `QuestionConfig` (user override)              |      ✗      |
| `DetachedMode` enum and detached-policy cascade              |      ✗      |
| `--non-interactive` CLI flag                                 |      ✗      |

When [RFD 049] lands in full, it strictly extends this work: the user-override
layer merges on top of the tool-level default, and the `DetachedMode` cascade
decides what an `auto`/`defaults`/`deny` policy does with an `exclusive`
question.
`ask_user` needs no code changes at that point.

## Drawbacks

- **One more built-in tool to maintain.** The tool itself is small but adds a
  surface the assistant can call, with new failure modes to test.
- **Unclear pedagogical signal for the LLM.** Providers sometimes over-use new
  tools.
  `ask_user` should only fire when the assistant genuinely needs input the
  conversation doesn't supply.
  The tool description must discourage reflexive use ("When in doubt, answer the
  user's original question; do not call `ask_user` for clarification you can
  derive from context.").
- **Generic-field discipline cost.** Adding `exclusive` and `persistence` to
  `Question` and `prompt_label` to `QuestionConfig` enriches types that many
  tool implementations touch.
  The prompter and the coordinator's routing site are the chokepoints that honor
  these fields, but any future code path that consumes either type has to
  remember they exist.
  Mitigated by defaults that match today's behavior and snapshot tests covering
  each combination at the prompter level; not eliminated.
- **Widening `exclusive`'s gate to block `target = "assistant"`.** The current
  code routes a question explicitly configured for the assistant to the inquiry
  backend regardless of any flag.
  After this RFD, a user-configured `target = "assistant"` on a tool that ships
  `exclusive: true` becomes an error instead.
  This is the right semantics for `exclusive` ("human-or-nothing") but is a
  behavior change for any tool that later opts into `exclusive: true` while
  users have already pinned `target = "assistant"` for its questions.
  No tool other than `ask_user` is in that state today; the [RFD 049]
  per-question user override provides the escape hatch when it lands.
- **Bare `-T` does not disable `ask_user`.** Because `ask_user` ships with
  `allow_toggle: IfNamed` (per [RFD 081]), passing bare `-T` (disable-all)
  disables every other tool while leaving `ask_user` callable.
  Users who want "no tools and no interactive prompts" for a turn have no single
  flag for it today; that case is deferred to a future bulk-disable-all flag
  (informally `-TT`), which would mean "disable everything including built-in
  tools."
  Until then, `-T -T ask_user` (bulk disable plus the named exception) is the
  workaround.
  Treating `ask_user` as a core conversational capability rather than a tool
  that bulk-disable should silently strip is deliberate — the assistant calling
  `ask_user` is the only in-band way for it to surface a question mid-turn.

## Alternatives

### Assistant-emitted inquiry directive (no tool)

Have the assistant emit an inquiry through a structured-output schema or a
special control token, with the turn loop pausing on detection.
Rejected: provider-specific (structured output support is uneven), mixing
structured output with tool-call streaming is fragile, and the free agentic-loop
continuation offered by tool calls disappears.

### Add `QuestionTarget::UserOnly`

Rejected, consistent with [RFD 019]'s own rejection of this variant.
Exclusivity belongs on `Question` (per [RFD 049]), not as a routing target.

### Name-specific guards in the coordinator

Short-circuit `InquiryBackend` routing when the tool's name is `ask_user`, or
reject `target = "assistant"` only when the tool name is `ask_user`.
Rejected: adds named exceptions to the coordinator that have to be torn out
again as soon as another tool wants the same human-only contract.
The generic `Question.exclusive` flag and the coordinator's `if
question.exclusive && resolved_target == Assistant: fail` check achieve the same
result without name-checking, and apply to any future tool that opts in.

### `Attribution` enum on `QuestionConfig` (typed display + provenance)

An earlier variant of this RFD proposed `QuestionConfig::attribution: enum {
Tool, Assistant }` that drove both the prompt's "who's asking" header and the
persisted `InquirySource`.
Rejected: conflates UI label with audit provenance.
A user-overridable config field that influences persisted `InquirySource` makes
provenance a styling choice — any config layer could relabel a database tool's
questions as assistant-sourced in the persisted record, which defeats the audit
purpose [RFD 005] motivates inquiry events with.
This RFD splits the concerns: `prompt_label` is display-only and
user-overridable; persisted provenance is derived from tool metadata (see [RFD
082]) and not user-overridable.

### `ask_user`-specific fields instead of generic enrichments

Scope `exclusive` and `persistence` to `ask_user` only — either as a parallel
type or as fields that exist but are only read on the `ask_user` path.

Rejected: other tool authors want the same expressive features.
A `git` tool asking "force push?" should be able to opt into `persistence:
None`.
A high-risk database tool should be able to mark its destructive confirmation
`exclusive: true`.
The type and the rendering layer evolve once; every consumer benefits.
The cost is that nothing at the type level forces a caller to consider the new
fields — the discipline is enforced by sensible defaults that match today's
behavior and by snapshot tests at the prompter layer.

### Separate `AssistantInquiry` type alongside `jp_tool::Question`

Introduce a parallel type — `AssistantInquiry` in `jp_conversation` — plus an
`AssistantTool` trait and a dedicated coordinator branch.
Type-level correctness improves: assistant inquiries cannot be accidentally
rendered through the tool-question path because the types refuse to compile that
way.

Rejected: `ask_user` is itself a tool.
Its questions are *tool* questions in the architectural sense — they originate
in tool execution, ride the tool-call wire transport, consume the same prompt
machinery, and their answers feed back through the tool-result envelope.
The differences from ordinary tool-questions (no Y/N persistence, distinct
visual label, no LLM auto-resolution) are all *policy* choices that other tools
could plausibly want.
Forking the type would create two near-identical paths whose drift is a
long-running maintenance liability, and would deny those policy choices to every
non-`ask_user` tool.

### Built-in tool allow-list for configurable fields

Declare a per-built-in allow-list of `QuestionConfig` (and broader
`PartialToolConfig`) fields that users may override.
Anything not on the list becomes a hard config error.
This would let `ask_user` reject `target = "assistant"` at config-resolution
time without a runtime check or a special `exclusive` interaction.

Rejected for this RFD as out of scope.
The allow-list is the right long-term shape for built-in tool configurability,
but it is a system-wide change affecting every built-in and deserves its own
RFD.
The narrow fix this RFD ships (`exclusive` widened to block `target =
"assistant"`) is sufficient for `ask_user` and reuses an existing generic
mechanism.

## Non-Goals

- **Assistant-to-assistant escalation or reasoning trails.** This RFD does not
  address sub-agent reasoning or escalation from assistant to user on rejection.
  Those are orthogonal concerns.
- **Full [RFD 049] Phase 1 scope.** This RFD implements only
  `jp_tool::Question.exclusive` and the single coordinator consumer needed for
  `ask_user`.
  The `QuestionConfig` user override, the `DetachedMode` cascade, and the
  `--non-interactive` CLI flag remain [RFD 049]'s responsibility.
- **The baseline unified recording lifecycle.** Recording every
  `Outcome::NeedsInput` round-trip as an `InquiryRequest`/ `InquiryResponse`
  pair is [RFD 082]'s responsibility. 083 only contributes `ask_user`-specific
  provenance (the `inquiry_source()` override) and the cancellation events for
  its new fail-fast routing paths and static-answer validation.
- **Built-in tool aliasing.** Users cannot alias `ask_user` under a different
  config key (the built-in executor lookup uses the config key, not
  `ToolSource::Builtin { tool }`).
  This is unchanged from today and affects every built-in; a future RFD can
  address aliasing uniformly.
- **Generic allow-list for built-in tool configurability.** A per-built-in
  declaration of which `QuestionConfig` (or broader `PartialToolConfig`) fields
  users may override is the right long-term shape for built-in configurability,
  but it is out of scope for this RFD.
- **Structured-output answers.** `answer_type` is limited to `boolean`,
  `select`, and `text` — the set already supported by `ToolPrompter`.
  Structured answers are out of scope.

## Risks and Open Questions

- **Misuse by the assistant.** If the LLM calls `ask_user` frequently and
  inappropriately, the experience degrades into a chatbot asking permission for
  everything.
  The tool description and built-in config defaults must discourage reflexive
  use.
  Real-world usage should be monitored during rollout.
- **Interaction with [RFD 049]'s detached policy.** This RFD pre-implements a
  subset of [RFD 049] Phase 1 (`Question.exclusive` + the single coordinator
  consumer).
  When [RFD 049] lands in full, the `DetachedMode` cascade decides what
  `auto`/`defaults`/`deny` do with an `exclusive` question.
  `ask_user` inherits that behavior with no code change — the `exclusive: true`
  default on its question is already in place.
- **Scope creep into [RFD 049].** By pre-implementing `exclusive`, this RFD
  takes on work that [RFD 049] owns.
  The risk is coordinating changes if [RFD 049] evolves the field's shape (type,
  default, semantics) before merging.
  Mitigation: keep the scope as narrow as possible — one field, the routing
  checks, no user-facing config — so any [RFD 049] divergence is a small fix.
  The user-override gap is further bounded by `ask_user` being the only
  `exclusive: true` emitter during this window; the first third-party tool that
  wants `exclusive` makes [RFD 049]'s override layer a hard dependency for
  itself.
- **Coordination with [RFD 081].** RFD 081 supplies the `Enable { state,
  allow_toggle }` shape that `ask_user` is registered with.
  If RFD 081's field names or `allow_toggle` variants change before merge, the
  single-line registration in `builtins::all()` shifts accordingly.
  No deeper coupling.
- **Hard dependency on [RFD 082].** 082 is a hard prerequisite for 083. 083
  widens the existing `InquiryQuestion` type (which lives in
  `jp_conversation::event::inquiry` today, introduced by [RFD 005]), adds
  `CancellationReason::InvalidStaticAnswer` to 082's `Cancelled` enum, overrides
  `BuiltinTool::inquiry_source()` (the hook 082 adds), and emits
  `InquiryResponse::Cancelled` events for 083's new routing paths and the
  static-answer validation failure via 082's recording lifecycle. 083 reuses
  082's `NoPromptBackend` and `AssistantRoutingDenied` variants — 082 ships
  them as part of its own `AnswerType::Secret` routing guard. 083 cannot ship
  before 082; the gate is enforced via `Requires` at promotion time.
- **Interaction with [RFD 018]'s `Prompt` enum.** [RFD 018] is in Discussion.
  Its `Prompt::ToolQuestion` variant will carry `ask_user`'s question with no
  special casing.
  No [RFD 018] blocker for this work.
- **Sensitive data exposure.** `ask_user` answers are returned to the LLM and
  (once [RFD 082] lands) persisted in the conversation stream.
  Do not use `ask_user` to collect passwords, API keys, SSH passphrases, or
  similar secrets unless the user intentionally wants those values sent to the
  model and stored on disk.
  The tool description should mention this; a future RFD may add a `sensitive:
  bool` flag that masks the answer in the persisted event.

## Implementation Plan

### Phase 1: Generic enrichments

Touches generic types and the prompter/coordinator.
No `ask_user` code yet.
No user-visible change for existing tool-questions when fields take their
defaults.

Depends on [RFD 082] being implemented first (the recording lifecycle and the
`BuiltinTool::inquiry_source()` hook originate there; the existing
`InquiryQuestion` shape that step 5 below widens already lives in
`jp_conversation::event::inquiry`, introduced by [RFD 005]).

1. Rename `jp_tool::Question.pre_amble` to `Question.context` (and the existing
   builder `Question::with_preamble` to `Question::with_context`), then add
   `exclusive: bool` and `persistence: AnswerReusePolicy` fields to
   `jp_tool::Question`:
   
   - `exclusive`: `#[serde(default, skip_serializing_if = "is_false")]`.
     Existing serialized `Question` payloads deserialize as `exclusive: false`.
   - `persistence`: variants `None` and `Turn`, default `Turn`, with
     `#[serde(default, skip_serializing_if = "AnswerReusePolicy::is_turn")]`.
     Older payloads deserialize as `Turn` (today's behavior).
   - Update the in-crate constructors (`Question::text`, `Question::boolean`,
     `Question::select`) to initialize the new fields to their defaults.
   - Add builder methods `Question::with_exclusive(bool)` and
     `Question::with_persistence(AnswerReusePolicy)` so callers outside
     `jp_tool` can set the new fields.
     `Question` is `#[non_exhaustive]`, so struct literals are not available
     outside the crate — without builders, Phase 2's `AskUser` (which lives in
     `jp_llm`) could not author a `Question` with `exclusive: true` and
     `persistence: None`.

2. Add the `prompt_label: Option<String>` field on `QuestionConfig`:
   
   - Add the field to `QuestionConfig` and the matching optional field on
     `PartialQuestionConfig`.
   - Update the manual `PartialConfigDelta` and `ToPartial` impls for
     `QuestionConfig` (see `crates/jp_config/src/conversation/tool.rs`) to cover
     the new field.
   - Regenerate the affected schema snapshots under
     `crates/jp_config/src/snapshots/` and any taplo / workspace schema fixtures
     that exercise tool-question config.
   - Add config-merging tests covering the field across the layered config flow
     (builtin default → user TOML → CLI delta).

3. Change the built-in tool injection in `apply_cli_config`
   (`crates/jp_cli/src/cmd/query.rs:1024-1031`) from
   `.entry(name).or_insert(config)` to a merge-as-lower-priority operation.
   Built-in defaults must merge *under* any user config in the same tool's
   namespace, so a user overriding a single field — for example
   `conversation.tools.ask_user.questions.answer.prompt_label` — does not lose
   the rest of the built-in's `source`, `enable`, `run`, `result`, and `style`
   defaults.
   The fix is generic; it applies to every entry returned by `builtins::all()`,
   not only `ask_user`.
   
   Built-in configs are lower-priority defaults; user config under a built-in's
   namespace overlays *on top of* the built-in's defaults so single-field
   overrides (the common case) inherit the rest.
   Users *may* also override structural fields including `source`, `parameters`,
   and `command` — changing `source` shadows the built-in executor and routes
   through the selected source path instead.
   This is unsupported in the sense that the user owns the consequences (a
   `local`-sourced `ask_user` no longer runs the built-in code), but it is not
   blocked.
   The reserved capability lives in the layering itself: built-in defaults
   always merge under, never replace.
   
   A partial override of `source` does not implicitly clear the built-in's other
   structural fields: the user's executor still runs against the built-in's
   `parameters` schema, `questions`, `style`, and `run`/`result` defaults unless
   those are explicitly overridden as well.
   Users replacing `source` typically also need to override `parameters` (and
   any other built-in-specific fields) to match their executor's contract.

4. Change `ToolPrompter::prompt_question` to take an optional pre-resolved
   prompt label alongside the question:
   
   ```rust
   pub fn prompt_question(
       &self,
       question: &Question,
       prompt_label: Option<&str>,
   ) -> Result<QuestionResult, Error>;
   ```
   
   Label resolution stays in the coordinator: it reads
   `config.questions[question.id].prompt_label.as_deref()` and passes the result
   through.
   When `None` (the default for existing tool-questions), the prompter renders
   the question as it does today (no "who's asking" header).
   This preserves the "no user-visible change for defaults" property: only
   `ask_user` and any other future tool that registers a `prompt_label` triggers
   the header rendering.
   The prompter also honors `question.persistence` — suppresses the `Y`/`N`
   "remember for turn" options for booleans when `persistence ==
   AnswerReusePolicy::None`.

5. Widen `jp_conversation::InquiryQuestion` with the persisted-side companions:
   
   - Add `context: Option<String>`, `exclusive: bool`, and `persistence:
     InquiryAnswerReusePolicy` fields with default-preserving serde attributes.
     `context` is a brand-new field on the persisted type (no legacy `pre_amble`
     to migrate; the field name mirrors the `jp_tool::Question` rename in step
     1).
   - Define `InquiryAnswerReusePolicy` in `jp_conversation::event::inquiry`
     (variants `None` and `Turn`, default `Turn`, `#[serde(rename_all =
     "snake_case")]`).
   - At 082's recording boundary (`tool_question_to_inquiry_question` or
     equivalent), convert from `jp_tool::Question` to
     `jp_conversation::InquiryQuestion` including the new fields, and from
     `jp_tool::AnswerReusePolicy` to
     `jp_conversation::InquiryAnswerReusePolicy`.
   - Tests: legacy `InquiryQuestion` JSON (no new fields) deserializes with the
     documented defaults; a `Question` constructed with `exclusive: true,
     persistence: None` round-trips to the persisted shape with the expected
     fields set.

6. Coordinator-side answer handling in `handle_tool_result`:
   
   - **Static-answer validation.** At 082's static-answer short-circuit (where
     `QuestionConfig.answer` is applied as the resolved answer for a
     `NeedsInput` question), validate the configured value against the in-flight
     question's `answer_type` and — for `select` — `options` *before* applying
     it.
     On mismatch, the coordinator synthesizes a tool-level error response naming
     `QuestionConfig.answer` as the source (e.g.
     `"<tool_name>: the configured
     conversation.tools.<tool>.questions.<id>.answer value does not match the
     question's answer_type. Update the configuration; do not retry."`), and
     records the corresponding inquiry response as `InquiryResponse::Cancelled {
     reason: InvalidStaticAnswer }` via 082's lifecycle.
     Source-aware validation lives at the boundary that knows the source; the
     executor's own type check (defensive, generic wording) remains as a
     fallback for any other routing path.
   - **Cache-persistence gate.** In the cached-answer short-circuit, skip the
     cache lookup when `question.persistence == AnswerReusePolicy::None`. 082
     ships the cache lookup unconditional (today's behavior); this RFD adds the
     persistence predicate so a `persistence: None` question is re-asked every
     time.
     The write side of the cache uses the same predicate.

7. Widen the `exclusive` routing checks in
   `ToolCoordinator::handle_tool_result`, and add the corresponding emit sites
   for 082's recording lifecycle:
   
   - **Add `CancellationReason::InvalidStaticAnswer`** to 082's `Cancelled` enum
     in `jp_conversation::event::inquiry`. 082 documents the enum as open to
     extension; 083 adds this single variant for the static-answer validation
     failure in step 6 (a `QuestionConfig.answer` that does not match the
     question's `answer_type` or `options`). 083 reuses 082's `NoPromptBackend`
     and `AssistantRoutingDenied` variants for the routing emit sites below —
     082 ships those as part of its own `AnswerType::Secret` routing guard.
   - **Before routing to the inquiry backend on no-TTY** (existing fallback
     path): check `question.exclusive` first; if set, synthesize a tool-level
     error response with the message `"<tool_name> cannot run because no
     interactive terminal is available. Do not retry this tool call in this
     turn; continue without user input or explain what information is
     missing."`, and record `InquiryResponse::Cancelled { reason:
     NoPromptBackend }` via 082's lifecycle.
   - **Before routing to the inquiry backend for explicit `target =
     "assistant"`**: check `question.exclusive` first; if set, synthesize a
     tool-level error response with the message `"<tool_name> requires a human
     answer and cannot be routed to the assistant. Do not retry this tool call
     in this turn."`, and record `InquiryResponse::Cancelled { reason:
     AssistantRoutingDenied }`.
     Both checks are generic on `question.exclusive`; no tool-name branching.
     The `Outcome::Error { transient }` flag is dropped when converting to
     `ExecutionOutcome`, so retryability is communicated by the error message
     wording, not by a flag.

Tests: snapshot tests at the prompter level covering each `(answer_type ×
persistence × prompt_label)` combination; unit tests for both `exclusive`
routing checks (no-TTY fallback and explicit `target = "assistant"`) asserting
the synthesized error response *and* the recorded `Cancelled` reason; a unit
test for step 6's static-answer validation asserting that a type-mismatched
`QuestionConfig.answer` produces a source-aware error response and a recorded
`Cancelled { reason: InvalidStaticAnswer }` event; a unit test that
`persistence: None` bypasses the cache lookup on both read and write sides; a
backward-compat test that deserializes a pre-083 `Question` JSON (without
`exclusive` / `persistence` fields) and asserts the defaults take effect.

Can be merged independently of Phase 2, but only after 082 has landed.

### Phase 2: `ask_user` built-in tool

Add `jp_llm::tool::builtin::ask_user::AskUser` implementing `BuiltinTool`,
authoring its `Question` with `exclusive: true` and `persistence:
AnswerReusePolicy::None`.
Override `BuiltinTool::inquiry_source()` (the hook [RFD 082] introduces) to
return `InquirySource::Assistant`, so the recording site 082 established
surfaces `ask_user`'s exchanges with the correct provenance.
Add the `ask_user()` config entry to `jp_cli::cmd::query::tool::builtins::all()`
with `enable: Enable { state: true, allow_toggle: IfNamed }` (per [RFD 081]),
`questions.answer.prompt_label = Some("Assistant".to_owned())`, a long-form
`description`, and the full `parameters` schema (see [Tool
configuration](#tool-configuration)).
Register the builtin in `handle_turn`.
Tests for argument validation, the `NeedsInput → Success` round-trip, the
JSON-encoded success body, that `tool_definitions()` exposes the expected
`description` and parameter schema to providers, and that
`inquiry_source("ask_user")` returns `InquirySource::Assistant` so 082's
recording site produces the right provenance.

Depends on Phase 1 (the enrichments must be in place so `ask_user` can author
its question correctly, and so both `exclusive` routing checks handle the
non-TTY and explicit-assistant cases before the tool is registered), on [RFD
081] for the `Enable` shape, and on [RFD 082] for the
`BuiltinTool::inquiry_source()` hook that this phase overrides.
**Not safe to ship without Phase 1**: without the `exclusive` routing checks the
non-interactive case auto-resolves via the inquiry backend (the worst possible
behavior for `ask_user`), and `target = "assistant"` would silently route the
assistant's own question to a sub-agent.

## References

- [RFD 005] — defines `InquirySource` and inquiry event recording rules;
  consumed by this RFD as the existing inquiry-event infrastructure.
- [RFD 028] — the inquiry coordinator this RFD reuses.
- [RFD 034] — defines the current `QuestionTarget` shape.
- [RFD 081] — decomposes `Enable` into `{ state, allow_toggle }`; `ask_user`
  registers with the shape RFD 081 introduces.
- [RFD 082] — unified inquiry event recording; hard prerequisite for this RFD.
  082 introduces the recording lifecycle and the `BuiltinTool::inquiry_source()`
  hook that `ask_user` overrides for `Assistant`-sourced persistence.
  This RFD widens the existing `InquiryQuestion` shape (in
  `jp_conversation::event::inquiry`, introduced by [RFD 005]) with
  `context`/`exclusive`/`persistence`, and adds the
  `CancellationReason::InvalidStaticAnswer` variant to 082's `Cancelled` enum.
  083 reuses 082's `NoPromptBackend` and `AssistantRoutingDenied` variants for
  its routing fail-fast emits.
  Also amends [RFD 005].
- [RFD 018] — future `Prompt` enum that will carry this tool's question without
  special casing.
- [RFD 049] — defines the full `exclusive` flag and detached-policy cascade
  that this RFD pre-implements a subset of.
- [RFD 055] — tool groups and the broader `Enable` variant restructuring; no
  direct interaction now that RFD 081 supplies the `Enable` shape.
- [RFD 019] — abandoned; referenced for the original `QuestionTarget::UserOnly`
  rejection rationale.
- GitHub issue [#311] — adjacent user demand for richer tool-controlled
  permission UX (custom prettyprinting, multi-step approval workflows). 083 does
  not directly address this; it adds the generic tool-question enrichments
  (persistence, exclusivity, prompt label) that a future RFD for tool-controlled
  permissions could compose with.

[#311]: https://github.com/dcdpr/jp/issues/311
[RFD 005]: 005-first-class-inquiry-events.md
[RFD 018]: 018-typed-prompt-routing-enum.md
[RFD 019]: 019-non-interactive-mode.md
[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 034]: 034-inquiry-specific-assistant-configuration.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 055]: 055-tool-groups.md
[RFD 081]: 081-decompose-tool-enable-into-state-and-allow_toggle.md
[RFD 082]: 082-unified-inquiry-event-recording.md
