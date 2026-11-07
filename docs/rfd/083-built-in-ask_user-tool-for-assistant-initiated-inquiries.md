# RFD 083: Built-in ask_user Tool for Assistant-Initiated Inquiries

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-17
- **Summary**: Adds a built-in `ask_user` tool for the assistant to ask typed
  human-only questions mid-turn.

## Summary

This RFD adds a built-in `ask_user` tool that lets the assistant ask the user a
typed question (boolean, select, or text) mid-turn.
Answering through a tool call keeps the agentic turn loop running instead of
ending the turn and requiring a separate query.

## Motivation

When the assistant needs confirmation or a choice from the user ("should I
proceed?", "which of these two approaches do you prefer?", "what's the target
directory?"), it has no in-band mechanism to ask.
The turn ends with a natural-language question in the assistant's reply, the
user must respond in a new query (`jp query yes`), and the agentic loop stops.

The wire transport and routing machinery for an in-band solution already exist:

- [RFD 028] (Implemented) defines the inquiry system that routes tool-authored
  questions through `ToolCoordinator`, including `Outcome::NeedsInput { question
  }` with `AnswerType::{Boolean, Select, Text}` and a `default`.
- [RFD 034] (Implemented) adds `QuestionTarget` with `User` and
  `Assistant(PartialAssistantConfig)` variants, and the coordinator logic that
  prompts the user via `ToolPrompter` for `QuestionTarget::User`.
- [RFD 049] (Discussion) proposes an `exclusive` flag for inquiries that an LLM
  cannot meaningfully answer.
  This RFD pre-implements a minimal subset of that flag so `ask_user` can refuse
  LLM fallback without a tool-name check in the coordinator.
- [RFD 081] (Discussion) decomposes the tool `enable` field into `{ state,
  allow_toggle }`, letting `ask_user` be enabled by default while remaining
  disableable by name.
  This RFD adopts that shape directly.

The missing piece is a tool surface the assistant can call.
A built-in `ask_user` tool supplies it.

To support `ask_user`, this RFD also adds three generic question features that
other tools can opt into: a `persistence` policy on `Question` (disabling
"remember for turn" on high-risk confirmations), an `exclusive` flag on
`Question` (refusing LLM fallback when no TTY is available), and a display-only
`prompt_label` field on `QuestionConfig` for a "who's asking" header
(`"Assistant"` for `ask_user`).
It widens the persisted `InquiryQuestion` shape with the new `Question` fields
and plugs into the unified inquiry recording lifecycle defined in [RFD 082].

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

The user sees the question as an inline prompt using the same UI as
tool-authored questions, attributed to the assistant rather than to a tool.
The user's answer becomes the tool result, and the agentic loop continues.

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

These arguments map directly onto `jp_tool::AnswerType` and `jp_tool::Question`.
`context` renders above the question line, matching `ToolPrompter`'s behavior
for `Question.context` (renamed from `pre_amble` in this RFD; see
[Implementation Plan](#phase-1-generic-enrichments)).
Keeping `context` as a separate argument guarantees a single-line `question`
without string-splitting heuristics that could reorder context and question.

The tool validates its arguments on the first call and returns `Outcome::Error`
with an actionable message for the LLM when any check fails:

- `question` is missing or empty.
- `question` contains a newline (multi-line background belongs in `context`).
- `answer_type == "select"` and `options` is missing or empty.
- `options` is set when `answer_type != "select"`.
- `default` has the wrong type for `answer_type` (e.g. a string default with
  `answer_type: "boolean"`).
- `default` is not in `options` when `answer_type == "select"`.

On the second call, the tool validates the accumulated answer against
`answer_type` and `options` before returning `Outcome::Success`, returning
`Outcome::Error` if:

- The answer's JSON type does not match `answer_type` (defending against any
  routing path that produces a mistyped value).
- `answer_type == "select"` and the answer is not in `options`.

The executor's error message is generic ("the accumulated answer does not match
the requested answer type").
For the common cause of a mistyped answer, a `QuestionConfig.answer` value that
disagrees with `answer_type` or `options`, the coordinator's static-answer
short-circuit catches the mismatch earlier and names `QuestionConfig.answer` as
the source so the LLM does not retry a configuration error (see [Implementation
Plan](#phase-1-generic-enrichments)).

### Execution flow

`ask_user` is a `BuiltinTool`:

- First call (`answers` is empty): validates arguments and returns
  `Outcome::NeedsInput { question }`.
  The authored `Question` sets `exclusive: true` and `persistence:
  AnswerReusePolicy::None` (see [Generic enrichments to tool
  questions](#generic-enrichments-to-tool-questions)).
  The prompt's "who's asking" label is `"Assistant"`, taken from the tool's
  default `QuestionConfig.prompt_label` (see [Tool
  configuration](#tool-configuration)).
- Second call (`answers` contains the response keyed by question ID): validates
  the answer and returns `Outcome::Success { content: <JSON-encoded answer> }`.

The tool always emits a single question with `id: "answer"`.
This ID is the config key in `conversation.tools.ask_user.questions.answer.*`
(see [Tool configuration](#tool-configuration)), the turn-storage key for the
answer, and the lookup key for `prompt_label`.

The success payload is a typed JSON object rather than raw text, so the LLM
receives the type alongside the value:

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

This keeps `true` (boolean), `"true"` (select option), and `"true"` (text
answer) distinct even when the provider stringifies tool responses.

Between the two calls, the coordinator runs the standard inquiry flow:
`ToolPrompter` renders the question (using `prompt_label` for the header and
`persistence` to control the `Y`/`N` "remember for turn" option), stores the
answer in the turn, re-runs the tool with the accumulated answer, and returns a
`ToolCallResponse`.
Every `Outcome::NeedsInput` round-trip is recorded in the conversation stream
under [RFD 082]'s lifecycle; see [Persisted recording](#persisted-recording).

### Tool configuration

`jp_cli::cmd::query::tool::builtins::all()` registers `ask_user` alongside
`describe_tools`:

- `source: ToolSource::Builtin { tool: None }`.

- `enable: PartialEnableConfig { state: Some(true), allow_toggle:
  Some(AllowToggle::IfNamed) }`: enabled by default, unaffected by bare `-T`
  (disable-all), and disableable by name via `-T ask_user` on the CLI or
  `conversation.tools.ask_user.enable = { state = false }` in TOML.
  Using the table form in TOML preserves `allow_toggle = "if_named"` under [RFD
  081]'s partial-merge rules, whereas the boolean shorthand `enable = false`
  resets `allow_toggle` to `any`.

- `run: Unattended`, `result: Unattended`: the question itself is the user
  interaction, so no tool permission or result-delivery prompts are shown.

- `style.hidden: true`: suppresses tool-call and tool-result chrome in the
  terminal so only the prompt and answer appear in the conversation flow.

- `questions.answer.prompt_label = "Assistant"`: display-only default that
  renders `"Assistant"` as the prompt header (see [Generic
  enrichments](#generic-enrichments-to-tool-questions)).
  Persisted provenance is tracked separately via [RFD 082].

- `description`: long-form guidance on when to call `ask_user`, set on
  `description` (with no `summary`) so `ToolDocs::schema_description()` sends
  the full usage guard to the provider on every request:

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

- `parameters`: provider-valid JSON Schema (no `"type": "any"` or conditional
  `required`).
  Cross-field constraints are enforced at runtime in `AskUser::execute` and
  returned as `Outcome::Error` (see [Arguments](#arguments)).

| Parameter     | Type emitted            | Required | Notes                                                                         |
| ------------- | ----------------------- | -------- | ----------------------------------------------------------------------------- |
| `question`    | `string`                | yes      | The question text. Must be single-line; runtime rejects newlines.             |
| `context`     | `string`                | no       | Optional multi-line context rendered above the question.                      |
| `answer_type` | `string`                | no       | Emitted with `"enum": ["boolean", "select", "text"]`. Defaults to `"text"`.   |
| `options`     | `array` of `string`     | no       | Required at runtime when `answer_type == "select"` and forbidden otherwise.   |
| `default`     | `["boolean", "string"]` | no       | Multi-type to cover boolean defaults and string defaults (`select` / `text`). |

`AskUser` sets `exclusive: true` and `persistence: AnswerReusePolicy::None` on
the `Question` it constructs in Rust rather than in user config, matching [RFD
049]'s pattern for tool-authored defaults.

#### Supported `questions.answer` configuration

Because `ask_user` reuses `QuestionConfig`, all of its fields can be set in
config, though not all combinations are valid at runtime:

| Field                  | Supported for `ask_user`? | Notes                                                                                                                                                          |
| ---------------------- | ------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `answer`               | yes                       | Fixed answer; skips the prompt. Useful for tests and automation.                                                                                               |
| `target = "user"`      | yes                       | Default. Prompts the human.                                                                                                                                    |
| `target = "assistant"` | **rejected at routing**   | Blocked by `exclusive: true`, which rejects both no-TTY fallback and explicit assistant targeting (see [Non-interactive behavior](#non-interactive-behavior)). |
| `prompt_label`         | yes                       | Display-only header. Defaults to `"Assistant"`.                                                                                                                |

Rejecting `target = "assistant"` uses the generic coordinator check `if
question.exclusive && resolved_target == Assistant: fail` rather than branching
on the tool name.

#### Built-in aliasing

Like other built-in tools, `ask_user` cannot be aliased under another config key
(e.g. `source = "builtin.ask_user"` on a tool named `ask`): the built-in
executor lookup keys on the tool name rather than `ToolSource::Builtin { tool }`
and fails with `ToolError::NotFound`.

### Generic enrichments to tool questions

This RFD adds `exclusive` and `persistence` to `jp_tool::Question` and
`prompt_label` to `QuestionConfig`.
All three apply to any tool that emits questions:

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
preserve existing behavior.
Tools that opt in set `exclusive` and `persistence` at runtime on the in-flight
`Question`, or set `prompt_label` in their built-in `PartialToolConfig` entry
alongside `target` or a fixed `answer`.

The coordinator resolves `QuestionConfig::prompt_label.as_deref()` and passes it
with the in-flight `Question` to `ToolPrompter::prompt_question`.
The prompter renders the header when `prompt_label` is `Some`, omits it when
`None`, and honors `question.persistence`.

`prompt_label` is display-only and does not affect the persisted
`InquirySource`, which [RFD 082] derives from tool metadata rather than
user-overridable config.
Keeping them separate lets a user relabel the prompt header without altering the
audit record, and lets a tool declare its source without dictating UI.

#### Propagating to the persisted shape

[RFD 082] records inquiries using `InquiryQuestion` (in
`jp_conversation::event::inquiry`, introduced by [RFD 005]).
This RFD widens `InquiryQuestion` with the new source-side fields so the
conversation stream records the context shown to the user and the policy that
drove routing:

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
persisted-event crate does not depend on `jp_tool`.
The coordinator converts `jp_tool::AnswerReusePolicy` to
`jp_conversation::InquiryAnswerReusePolicy` when recording.

Field defaults preserve the existing wire format: `persistence` uses
`#[serde(default, skip_serializing_if = "InquiryAnswerReusePolicy::is_turn")]`
so legacy events deserialize as `Turn` and the default is omitted on the wire;
`exclusive` uses `#[serde(default, skip_serializing_if = "is_false")]`; and
`context` defaults to `None` and is omitted when absent.

### Persisted recording

[RFD 082] defines the unified recording lifecycle: every `Outcome::NeedsInput`
round-trip produces an `InquiryRequest` and `InquiryResponse` pair, cancelled or
refused inquiries record `InquiryResponse::Cancelled` (with `User`,
`BackendError`, `NoPromptBackend`, and `AssistantRoutingDenied` reasons, the
last two introduced for [RFD 082]'s `AnswerType::Secret` routing guard),
turn-cached answers are tracked separately, and `BuiltinTool::inquiry_source()`
defaults to `InquirySource::Tool { name }`.

This RFD plugs into that lifecycle and contributes:

- `AskUser` overrides `BuiltinTool::inquiry_source()` to return
  `InquirySource::Assistant`, persisting `ask_user` exchanges with assistant
  provenance.
- `InquiryQuestion` gains `context`, `exclusive`, and `persistence` (see
  [Propagating to the persisted shape](#propagating-to-the-persisted-shape)).
- `CancellationReason` gains `InvalidStaticAnswer` for a configured
  `QuestionConfig.answer` that does not match the in-flight question's
  `answer_type` or `options`.
- The coordinator records `InquiryResponse::Cancelled` on its new fail-fast
  paths, reusing [RFD 082]'s `NoPromptBackend` and `AssistantRoutingDenied`
  variants for the `exclusive: true` guards:

| Condition                                              | Persisted reason         |
| ------------------------------------------------------ | ------------------------ |
| `target = "user"`, no TTY, `exclusive = true`          | `NoPromptBackend`        |
| `target = "assistant"`, `exclusive = true`             | `AssistantRoutingDenied` |
| `QuestionConfig.answer` invalid for the question shape | `InvalidStaticAnswer`    |

Because the `inquiry_source()` hook, the `Cancelled` variant, the recording
lifecycle, and the `NoPromptBackend` and `AssistantRoutingDenied` reasons all
originate in [RFD 082], this RFD cannot ship before [RFD 082].

### Non-interactive behavior

When no TTY is available (the `is_tty` heuristic, `io::stdout().is_terminal()`,
returns `false`), `ToolCoordinator` routes `QuestionTarget::User` questions
through `InquiryBackend`, asking an LLM to answer the question.
For `ask_user`, auto-resolving with a sub-agent defeats the purpose of the tool:
the assistant called `ask_user` because it needs human input.
(If [RFD 049] later replaces `is_tty` with `/dev/tty` detection, `ask_user`
inherits the new behavior without code changes.)

**This RFD does not introduce a new `QuestionTarget` variant.** [RFD 019]'s
proposal of `QuestionTarget::UserOnly` was rejected because exclusivity is
orthogonal to target: it describes whether the target can be overridden when
unavailable, not who the target is.
[RFD 049] (Discussion) proposes an `exclusive` flag on the question plus a
detached-policy cascade for when no client is attached.

**This RFD pre-implements a minimal subset of [RFD 049] Phase 1**: the
`exclusive: bool` field on `jp_tool::Question` (default `false`; see [Generic
enrichments](#generic-enrichments-to-tool-questions)) and the coordinator
routing that consumes it.

`exclusive: true` marks a question as human-only.
The coordinator enforces this by refusing two routing paths:

1. **No-TTY fallback to the inquiry backend.** When a user-targeted question has
   `exclusive == true` and no TTY is available, the coordinator fails the tool
   instead of routing through `InquiryBackend`, returning an error that tells
   the LLM not to retry (`"<tool_name> cannot run because no interactive
   terminal is available. Do not retry this tool call in this turn; continue
   without user input or explain what information is missing."`).
2. **Explicit `target = "assistant"`.** When a question has `exclusive == true`
   and the resolved `QuestionConfig.target` is `Assistant`, the coordinator
   fails the tool before routing (`if question.exclusive && resolved_target ==
   Assistant: fail`).
   This check applies to any tool that sets `exclusive: true`, avoiding
   tool-name guards in the coordinator.

`exclusive` does **not** block a configured `QuestionConfig.answer`: pinning a
static answer in config opts out of routing altogether.

`ask_user` sets `exclusive: true` on its question.
No other tool sets the flag initially, so behavior for all other tools is
unchanged.

**Resolution precedence for tool questions:**

| Configuration                                  | Routing outcome                                      |
| ---------------------------------------------- | ---------------------------------------------------- |
| `QuestionConfig.answer` set                    | Use the configured answer. Routing skipped entirely. |
| `target = "user"`, TTY available               | Prompt the user (existing behavior).                 |
| `target = "user"`, no TTY, `exclusive = false` | Route to inquiry backend (existing behavior).        |
| `target = "user"`, no TTY, `exclusive = true`  | **Fail the tool** (new in this RFD).                 |
| `target = "assistant"`, `exclusive = false`    | Route to inquiry backend (existing behavior).        |
| `target = "assistant"`, `exclusive = true`     | **Fail the tool** (new in this RFD).                 |

Scope of the pre-implementation vs. [RFD 049] Phase 1 in full:

| [RFD 049] Phase 1 scope                                  | In this RFD |
| ------------------------------------------------------------ | :---------: |
| `exclusive: bool` on `jp_tool::Question` (tool default)      |      ✓      |
| Coordinator consumes `exclusive` for fail-vs-inquire routing |      ✓      |
| `exclusive` on `QuestionConfig` (user override)              |      ✗      |
| `DetachedMode` enum and detached-policy cascade              |      ✗      |
| `--non-interactive` CLI flag                                 |      ✗      |

When [RFD 049] lands in full, it extends this work without requiring changes to
`ask_user`: `QuestionConfig.exclusive` merges on top of the tool-level default
(letting a user set `exclusive = false` to allow `target = "assistant"`), and
the `DetachedMode` cascade governs how `auto`, `defaults`, and `deny` policies
handle an `exclusive` question when no TTY is attached.

## Drawbacks

- **One more built-in tool to maintain.** The tool is small, but adds a callable
  surface with new failure modes to test.
- **Model over-use.** Providers sometimes over-use new tools.
  `ask_user` should fire only when the assistant needs input the conversation
  does not provide.
  The tool description discourages reflexive use ("When in doubt, answer the
  user's original question; do not call `ask_user` for clarification you can
  derive from context.").
- **Generic-field discipline cost.** Adding `exclusive` and `persistence` to
  `Question` and `prompt_label` to `QuestionConfig` widens types touched by many
  tool implementations.
  `ToolPrompter` and the coordinator's routing logic are the chokepoints that
  honor these fields, but future consumers of either type must account for them.
  Defaults matching existing behavior and prompter snapshot tests mitigate this
  risk without eliminating it.
- **Widening `exclusive` to block `target = "assistant"`.** A user-configured
  `target = "assistant"` on a tool that sets `exclusive: true` becomes an error
  rather than routing to the inquiry backend.
  This matches the human-only contract of `exclusive`, but changes behavior for
  any tool that later opts into `exclusive: true` after users have configured
  `target = "assistant"` for its questions.
  Only `ask_user` sets `exclusive: true` initially, and [RFD 049]'s per-question
  user override will provide an escape hatch.
- **Bare `-T` does not disable `ask_user`.** Because `ask_user` ships with
  `allow_toggle: IfNamed` (per [RFD 081]), bare `-T` (disable-all) disables
  every other tool while leaving `ask_user` callable.
  Running a turn with no tools and no interactive prompts requires `-T -T
  ask_user` until a future bulk-disable-all flag (informally `-TT`) lands to
  disable all tools including built-ins.
  Treating `ask_user` as a core conversational capability that bare `-T` does
  not strip is deliberate: calling `ask_user` is the assistant's only in-band
  way to ask a question mid-turn.

## Alternatives

### Assistant-emitted inquiry directive (no tool)

Have the assistant emit an inquiry through a structured-output schema or a
special control token, pausing the turn loop on detection.
Rejected: structured-output support varies across providers, mixing structured
output with tool-call streaming is fragile, and tool calls already provide
agentic-loop continuation for free.

### Add `QuestionTarget::UserOnly`

Rejected, matching [RFD 019]'s rejection of this variant.
Exclusivity belongs on `Question` (per [RFD 049]), not as a routing target.

### Name-specific guards in the coordinator

Short-circuit `InquiryBackend` routing or reject `target = "assistant"` only
when the tool name is `ask_user`.
Rejected: adds tool-name exceptions to the coordinator that would have to be
removed as soon as another tool needs the same human-only contract.
The generic `Question.exclusive` flag and `if question.exclusive &&
resolved_target == Assistant: fail` check achieve the same result for any tool
that opts in.

### `Attribution` enum on `QuestionConfig` (typed display + provenance)

An earlier draft proposed `QuestionConfig::attribution: enum { Tool, Assistant
}` to drive both the prompt's "who's asking" header and the persisted
`InquirySource`.
Rejected: conflates UI labeling with audit provenance.
If a user-overridable config field controlled `InquirySource`, any config layer
could relabel a database tool's questions as assistant-sourced in the persisted
stream, defeating the audit guarantee in [RFD 005].
This RFD separates the two: `prompt_label` is display-only and user-overridable,
whereas persisted provenance comes from tool metadata (see [RFD 082]) and cannot
be overridden in config.

### `ask_user`-specific fields instead of generic enrichments

Scope `exclusive` and `persistence` to `ask_user` only, either on a parallel
type or as fields read only on the `ask_user` path.

Rejected: other tools need the same policies.
A `git` tool asking "force push?" can opt into `persistence: None`, and a
database tool can mark a destructive confirmation `exclusive: true`.
Evolving the shared type and prompter once benefits every tool, while sensible
defaults and prompter snapshot tests keep existing callers unaffected.

### Separate `AssistantInquiry` type alongside `jp_tool::Question`

Introduce a parallel `AssistantInquiry` type in `jp_conversation`, an
`AssistantTool` trait, and a dedicated coordinator branch so assistant inquiries
cannot compile against the tool-question path.

Rejected: `ask_user` is a tool.
Its questions originate in tool execution, use the tool-call wire transport and
prompt machinery, and return answers through the tool-result envelope.
Its differences from ordinary tool questions (no `Y`/`N` persistence, distinct
header label, no LLM auto-resolution) are policy choices that other tools can
also use.
Forking the type would duplicate the pipeline and deny those policies to other
tools.

### Built-in tool allow-list for configurable fields

Declare a per-built-in allow-list of `QuestionConfig` (and `PartialToolConfig`)
fields that users may override, rejecting disallowed overrides at
config-resolution time (including `target = "assistant"` on `ask_user`).

Rejected as out of scope.
A per-built-in allow-list affects every built-in tool and belongs in its own
RFD.
Having `exclusive` block `target = "assistant"` is sufficient for `ask_user` and
reuses an existing generic mechanism.

## Non-Goals

- **Assistant-to-assistant escalation or reasoning trails.** Sub-agent reasoning
  and escalation from assistant to user on rejection are orthogonal concerns.
- **Full [RFD 049] Phase 1 scope.** This RFD implements only
  `jp_tool::Question.exclusive` and the coordinator checks needed for
  `ask_user`.
  The `QuestionConfig` user override, the `DetachedMode` cascade, and the
  `--non-interactive` CLI flag remain in [RFD 049].
- **Baseline unified recording lifecycle.** Recording every
  `Outcome::NeedsInput` round-trip as an `InquiryRequest` and `InquiryResponse`
  pair belongs to [RFD 082].
  This RFD contributes only `ask_user`'s `inquiry_source()` override, the
  `InquiryQuestion` field additions, and the cancellation events for its routing
  and static-answer checks.
- **Built-in tool aliasing.** Built-in executor lookup uses the config key
  rather than `ToolSource::Builtin { tool }`, so `ask_user` cannot be aliased
  under another name.
- **Generic allow-list for built-in tool configurability.** Declaring which
  `QuestionConfig` or `PartialToolConfig` fields each built-in allows users to
  override is deferred to a future RFD.
- **Structured-output answers.** `answer_type` is limited to `boolean`,
  `select`, and `text`, matching the types supported by `ToolPrompter`.

## Risks and Open Questions

- **Misuse by the assistant.** Frequent or unnecessary `ask_user` calls degrade
  the experience into constant permission prompts.
  The tool description discourages reflexive use, and real-world usage should be
  monitored during rollout.
- **Interaction with [RFD 049]'s detached policy.** When [RFD 049] lands in
  full, its `DetachedMode` cascade will govern what `auto`, `defaults`, and
  `deny` do with an `exclusive: true` question.
  Because `ask_user` already sets `exclusive: true`, it will inherit that
  behavior without code changes.
- **Scope overlap with [RFD 049].** Pre-implementing `exclusive` takes on a
  piece of [RFD 049].
  If [RFD 049] changes the field's type, default, or semantics before merging,
  this code must be updated to match.
  Keeping the pre-implementation minimal (one field and two routing checks, with
  no user-facing config) bounds any rework, and `ask_user` is the only
  `exclusive: true` emitter until [RFD 049] lands.
- **Coordination with [RFD 081].** [RFD 081] defines the `Enable { state,
  allow_toggle }` shape used to register `ask_user`.
  If its field or variant names change before merge, the registration in
  `builtins::all()` updates accordingly.
- **Hard dependency on [RFD 082].** This RFD depends on [RFD 082]'s recording
  lifecycle, `BuiltinTool::inquiry_source()` hook, and
  `InquiryResponse::Cancelled` enum (reusing `NoPromptBackend` and
  `AssistantRoutingDenied`, and adding
  `CancellationReason::InvalidStaticAnswer`).
  It cannot ship before [RFD 082]; the dependency is enforced via `Requires` at
  promotion time.
- **Interaction with [RFD 018]'s `Prompt` enum.** [RFD 018]'s
  `Prompt::ToolQuestion` variant will carry `ask_user`'s question without
  special casing, so [RFD 018] does not block this work.
- **Sensitive data exposure.** `ask_user` answers are returned to the LLM and
  persisted in the conversation stream once [RFD 082] lands.
  The tool description warns against collecting passwords, API keys, SSH
  passphrases, or other secrets; a future RFD may add a `sensitive: bool` flag
  to mask answers in persisted events.

## Implementation Plan

### Phase 1: Generic enrichments

Touches generic types, `ToolPrompter`, and `ToolCoordinator`, with no `ask_user`
code and no user-visible change when the new fields take their defaults.
Depends on [RFD 082] landing first.

1. Rename `jp_tool::Question.pre_amble` to `Question.context` (and
   `Question::with_preamble` to `Question::with_context`), then add `exclusive:
   bool` and `persistence: AnswerReusePolicy` to `jp_tool::Question`:

   - `exclusive`: `#[serde(default, skip_serializing_if = "is_false")]`, so
     existing serialized `Question` payloads deserialize as `exclusive: false`.
   - `persistence`: variants `None` and `Turn` (default `Turn`), with
     `#[serde(default, skip_serializing_if = "AnswerReusePolicy::is_turn")]` so
     existing payloads deserialize as `Turn`.
   - Initialize the new fields to their defaults in `Question::text`,
     `Question::boolean`, and `Question::select`.
   - Add `Question::with_exclusive(bool)` and
     `Question::with_persistence(AnswerReusePolicy)` builders.
     Because `Question` is `#[non_exhaustive]`, callers outside `jp_tool` (such
     as `AskUser` in `jp_llm`) cannot use struct literals to set `exclusive:
     true` and `persistence: None`.

2. Add `prompt_label: Option<String>` to `QuestionConfig`:

   - Add the field to `QuestionConfig` and `PartialQuestionConfig`.
   - Update the manual `PartialConfigDelta` and `ToPartial` implementations for
     `QuestionConfig` in `crates/jp_config/src/conversation/tool.rs`.
   - Regenerate the schema snapshots under `crates/jp_config/src/snapshots/` and
     any taplo or workspace schema fixtures that cover tool-question config.
   - Add config-merging tests across the layered config flow (built-in default
     → user TOML → CLI delta).

3. Change built-in tool injection in `apply_cli_config`
   (`crates/jp_cli/src/cmd/query.rs:1024-1031`) from
   `.entry(name).or_insert(config)` to merge built-in defaults *under* user
   config for every entry in `builtins::all()`.
   Overriding a single field (such as
   `conversation.tools.ask_user.questions.answer.prompt_label`) then preserves
   the built-in's remaining `source`, `enable`, `run`, `result`, and `style`
   defaults.

   Users may also override structural fields (`source`, `parameters`,
   `command`).
   Changing `source` shadows the built-in executor and routes through the
   configured source instead, while still inheriting any unoverridden
   `parameters`, `questions`, `style`, and `run`/`result` defaults from the
   built-in.
   Users who replace `source` are responsible for overriding `parameters` and
   other built-in-specific fields to match their executor's contract.

4. Update `ToolPrompter::prompt_question` to accept an optional pre-resolved
   prompt label alongside the question:

   ```rust
   pub fn prompt_question(
       &self,
       question: &Question,
       prompt_label: Option<&str>,
   ) -> Result<QuestionResult, Error>;
   ```

   The coordinator resolves
   `config.questions[question.id].prompt_label.as_deref()` and passes it to the
   prompter.
   When `None` (the default for existing tool questions), the prompter omits the
   "who's asking" header.
   When `question.persistence == AnswerReusePolicy::None`, the prompter also
   suppresses the `Y`/`N` "remember for turn" options on boolean questions.

5. Widen `jp_conversation::InquiryQuestion` with the persisted-side fields:

   - Add `context: Option<String>`, `exclusive: bool`, and `persistence:
     InquiryAnswerReusePolicy` with default-preserving serde attributes
     (`context` mirrors the `jp_tool::Question` rename in step 1).
   - Define `InquiryAnswerReusePolicy` in `jp_conversation::event::inquiry`
     (variants `None` and `Turn`, default `Turn`, `#[serde(rename_all =
     "snake_case")]`).
   - At [RFD 082]'s recording boundary (`tool_question_to_inquiry_question` or
     equivalent), convert `jp_tool::Question` and `jp_tool::AnswerReusePolicy`
     into their `jp_conversation` counterparts.
   - Add tests verifying that legacy `InquiryQuestion` JSON without the new
     fields deserializes to the defaults, and that a `Question` with `exclusive:
     true` and `persistence: None` round-trips to the persisted shape.

6. Update coordinator answer handling in `handle_tool_result`:

   - **Static-answer validation.** At [RFD 082]'s static-answer short-circuit,
     validate `QuestionConfig.answer` against the in-flight question's
     `answer_type` and (for `select`) `options` before applying it.
     On mismatch, synthesize a tool-level error response naming
     `QuestionConfig.answer` (e.g. `"<tool_name>: the configured
     conversation.tools.<tool>.questions.<id>.answer value does not match the
     question's answer_type. Update the configuration; do not retry."`) and
     record `InquiryResponse::Cancelled { reason: InvalidStaticAnswer }` via
     [RFD 082]'s lifecycle.
     Validating at the coordinator boundary identifies the config source; the
     executor's generic type check remains as a fallback for other routing
     paths.
   - **Cache-persistence gate.** Skip both reading and writing the per-turn
     answer cache when `question.persistence == AnswerReusePolicy::None`, so a
     `persistence: None` question is asked every time.

7. Add the `exclusive` routing checks in `ToolCoordinator::handle_tool_result`
   and their [RFD 082] cancellation events:

   - **Add `CancellationReason::InvalidStaticAnswer`** to [RFD 082]'s
     `Cancelled` enum in `jp_conversation::event::inquiry` for step 6's
     static-answer validation failure.
     The routing checks below reuse [RFD 082]'s `NoPromptBackend` and
     `AssistantRoutingDenied` variants.
   - **No-TTY fallback path:** if `question.exclusive` is `true`, synthesize a
     tool-level error response (`"<tool_name> cannot run because no interactive
     terminal is available. Do not retry this tool call in this turn; continue
     without user input or explain what information is missing."`) and record
     `InquiryResponse::Cancelled { reason: NoPromptBackend }`.
   - **Explicit `target = "assistant"` path:** if `question.exclusive` is
     `true`, synthesize a tool-level error response (`"<tool_name> requires a
     human answer and cannot be routed to the assistant. Do not retry this tool
     call in this turn."`) and record `InquiryResponse::Cancelled { reason:
     AssistantRoutingDenied }`.
     Both checks inspect `question.exclusive` rather than the tool name.
     Because `Outcome::Error { transient }` is dropped when converting to
     `ExecutionOutcome`, non-retryability is communicated in the error text.

Tests: prompter snapshot tests for each `(answer_type × persistence ×
prompt_label)` combination; unit tests for both `exclusive` routing checks
(no-TTY fallback and explicit `target = "assistant"`) asserting the error
response and recorded `Cancelled` reason; a unit test for static-answer
validation asserting the source-aware error and `Cancelled { reason:
InvalidStaticAnswer }` event; a unit test verifying that `persistence: None`
bypasses the turn cache on read and write; and a backward-compatibility test
deserializing pre-083 `Question` JSON without `exclusive` or `persistence`.

Phase 1 can merge independently of Phase 2 once [RFD 082] has landed.

### Phase 2: `ask_user` built-in tool

Implement `BuiltinTool` on `jp_llm::tool::builtin::ask_user::AskUser`,
constructing its `Question` with `exclusive: true` and `persistence:
AnswerReusePolicy::None`, and overriding `BuiltinTool::inquiry_source()` ([RFD
082]) to return `InquirySource::Assistant`.
Add the `ask_user()` config entry to `jp_cli::cmd::query::tool::builtins::all()`
with `enable: PartialEnableConfig { state: Some(true), allow_toggle:
Some(AllowToggle::IfNamed) }` ([RFD 081]), `questions.answer.prompt_label =
Some("Assistant".to_owned())`, the long-form `description`, and the `parameters`
schema (see [Tool configuration](#tool-configuration)).
Register the built-in in `handle_turn`.
Add tests covering argument validation, the `NeedsInput → Success` round-trip,
the JSON-encoded success body, the provider schema emitted by
`tool_definitions()`, and `inquiry_source("ask_user")` returning
`InquirySource::Assistant`.

Phase 2 depends on Phase 1, [RFD 081], and [RFD 082].
**Do not ship Phase 2 without Phase 1**: without the `exclusive` routing checks,
non-interactive runs would auto-resolve `ask_user` through the inquiry backend,
and `target = "assistant"` would silently route the assistant's question to a
sub-agent.

## References

- [RFD 005]: defines `InquirySource` and inquiry event recording rules.
- [RFD 028]: defines the inquiry coordinator reused by this RFD.
- [RFD 034]: defines `QuestionTarget`.
- [RFD 081]: decomposes `Enable` into `{ state, allow_toggle }`, used to
  register `ask_user`.
- [RFD 082]: unified inquiry event recording and hard prerequisite for this RFD.
  Introduces the recording lifecycle, `BuiltinTool::inquiry_source()`, and
  `InquiryResponse::Cancelled` (`NoPromptBackend`, `AssistantRoutingDenied`),
  which this RFD extends with `CancellationReason::InvalidStaticAnswer` and
  wider `InquiryQuestion` fields.
- [RFD 018]: future `Prompt` enum that will carry `ask_user` questions without
  special casing.
- [RFD 049]: defines the full `exclusive` flag and detached-policy cascade, a
  subset of which is pre-implemented here.
- [RFD 055]: tool groups and broader `Enable` restructuring; no direct
  interaction now that [RFD 081] supplies the `Enable` shape.
- [RFD 019]: abandoned; cited for the rejection rationale of
  `QuestionTarget::UserOnly`.
- GitHub issue [#311]: related demand for richer tool-controlled permission UX
  (custom formatting, multi-step approvals), which can build on the generic
  `persistence`, `exclusive`, and `prompt_label` additions here.

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
