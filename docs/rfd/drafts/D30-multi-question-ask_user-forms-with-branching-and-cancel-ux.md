# RFD D30: Multi-question ask_user forms with branching and cancel UX

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-17
- **Extends**: [RFD D27](D27-built-in-ask_user-tool-for-assistant-initiated-inquiries.md)

## Summary

Extend the `ask_user` built-in tool from [RFD D27] to collect multi-question
forms in a single tool call, with predicate-gated branching, a unified cancel
UX (`Reply` / `End Turn` / `Back`), and support for `boolean`, `select`,
`multi_select`, `text`, and `schema` answer types. The LLM fills in a
constrained meta-schema that defines the question flow; the coordinator walks
it, prompting the user and collecting answers.

## Motivation

[RFD D27] makes `ask_user` a single-question tool. In practice, user-facing
flows often need several related answers (e.g. "which directory, and with
what permissions?"). Today, the LLM collects them with multiple `ask_user`
calls, paying an LLM round-trip per call. For linear or branching forms
where the questions are predictable, this is wasteful — the LLM already
knows what it wants to ask.

A single `ask_user` call with multiple questions and predicate-gated
follow-ups collapses the round-trips. Combined with a typed-schema answer
([RFD D13]), it also unlocks lightweight user-facing forms (e.g. "give me
the structured config for this migration") without tool authors having to
build one-off prompts.

Independently, [RFD D27] left the cancel UX sparse: the user can answer or
Ctrl+C. This RFD fills in the discoverable options — `Reply`, `End Turn`,
`Back` — which become useful enough on their own that they apply to
single-question `ask_user` calls too.

## Design

### User-visible behavior

The assistant submits a list of questions; the user answers them in order,
with the ability to go back, cancel with a reply, or end the turn:

```jsonc
{
  "name": "ask_user",
  "arguments": {
    "questions": [
      {
        "id": "apply",
        "text": "Apply the proposed migration?",
        "answer_type": "boolean"
      },
      {
        "id": "env",
        "text": "Which environment?",
        "answer_type": "select",
        "options": ["staging", "production"],
        "when": { "question_id": "apply", "equals": true }
      },
      {
        "id": "note",
        "text": "Optional note for the migration log",
        "answer_type": "text",
        "when": { "question_id": "apply", "equals": true }
      }
    ]
  }
}
```

The user sees each prompt in sequence, prefixed with a progress indicator
(`[1/3]`, `[2/3]`, …). Questions whose `when` predicate evaluates false are
skipped silently.

### Tool arguments meta-schema

The `ask_user` tool's own argument schema describes what the LLM can submit.
[RFD D27]'s single-question shape becomes the common case where `questions`
contains one entry; this RFD expands that shape.

```jsonc
{
  "type": "object",
  "properties": {
    "questions": {
      "type": "array",
      "minItems": 1,
      "items": {
        "type": "object",
        "properties": {
          "id": { "type": "string" },
          "text": { "type": "string" },
          "answer_type": {
            "enum": ["boolean", "select", "multi_select", "text", "schema"]
          },
          "options": {
            "type": "array",
            "items": { "type": "string" }
          },
          "schema": { "type": "object" },
          "default": true,
          "when": {
            "type": "object",
            "properties": {
              "question_id": { "type": "string" },
              "equals": true
            },
            "required": ["question_id", "equals"]
          }
        },
        "required": ["id", "text", "answer_type"]
      }
    }
  },
  "required": ["questions"]
}
```

We control the branching model by constraining the meta-schema. LLMs fill it
in; they do not generate arbitrary JSON Schema. This prevents the rendering
complexity of generic `oneOf`/discriminator flows while still supporting
typical branching use cases.

Validation (at submit time, returning a structured error back to the LLM on
failure):

- `id` values are unique.
- `options` is required iff `answer_type` is `select` or `multi_select`.
- `schema` is required iff `answer_type` is `schema`.
- `when.question_id` references an `id` earlier in the list (no forward
  references; prevents cycles structurally).

### Branching: the `when` predicate

Each question has an optional `when` predicate:

```jsonc
{ "question_id": "<earlier id>", "equals": <value> }
```

At runtime, when the walker reaches a question, it evaluates the predicate
against the collected answers so far. If the predicate is absent or true,
the question is prompted. If false, the question is skipped and its slot in
the return map is `null`.

Only `equals` is supported in v1. Future extensions (`not_equals`, `one_of`,
nested predicates) are left to follow-up RFDs if real usage demands them.

### Cancel UX

Every prompt offers three cancel paths alongside the answer:

| Option     | Effect                                                        |
|------------|---------------------------------------------------------------|
| `Reply`    | Cancel the form, return to the LLM with a cancel marker;     |
|            | turn continues.                                               |
| `End Turn` | End the turn immediately. Same effect as Ctrl+C, but an       |
|            | explicit, discoverable option.                                |
| `Back`     | Re-prompt the previous *answered* question. Omitted on the    |
|            | first question.                                               |

The surface depends on the answer type:

- **Boolean**: inline-select extends the existing git-style keys:
  `y` / `Y` / `n` / `N` / `b` / `r` / `s`.
- **Select**: inline-select appends `b` / `r` / `s` to the option keys.
- **MultiSelect**: `inquire::MultiSelect` submits on Enter; Esc jumps to a
  cancel menu `[Back / Reply / End Turn]`.
- **Text**: a pre-prompt select `[Answer / Back / Reply / End Turn]` is
  shown first; `Answer` opens the text input.
- **Schema**: same pre-prompt select as Text; `Answer` opens a JSON editor
  (validated against the question's schema on submit).

The `Back` option is omitted when there is no prior answered question.

### Back navigation and answer invalidation

`Back` re-prompts the previous answered question with the current answer
pre-populated. On re-submit, the walker continues from that position,
**discarding all answers collected after it**. This keeps predicate
evaluation consistent: a changed answer at Q1 naturally re-gates Q2/Q3.

If the re-answer changes a predicate's branch (so a different set of later
questions applies), the walker discovers this as it re-walks forward.

### Return shape

The tool returns a JSON object mapping `question_id` → answer. Skipped or
`Back`-invalidated-but-not-re-answered questions map to `null`.

```jsonc
{
  "apply": true,
  "env": "production",
  "note": null
}
```

This gives the LLM a uniform, parseable shape. Skipped questions appear as
`null`, which the LLM can distinguish from absent questions by inspecting
the submitted `questions` list.

### Cancel return shape

On `Reply`, the tool returns a structured cancel marker so the LLM can
decide whether to proceed with partial information or re-ask:

```jsonc
{
  "cancelled": true,
  "answered": {
    "apply": true
  }
}
```

The `answered` map contains whatever was collected before the cancel.

On `End Turn`, the tool does not return — the turn ends before the tool
response is delivered. This matches the Ctrl+C path.

### `AnswerType::Schema` integration

This RFD adds the `Schema` variant to `jp_tool::AnswerType`:

```rust
pub enum AnswerType {
    Boolean,
    Select { options: Vec<String> },
    MultiSelect { options: Vec<String> },
    Text,
    Schema { schema: Value },
}
```

The variant is a small addition that can land independently; if [RFD D13]
(which defines the same variant for tool-authored questions) lands first,
this RFD consumes the existing definition instead of introducing one.

The v1 renderer is a JSON editor pre-populated with the question's
`default` (if any) and validated against the schema on submit. Rich
schema-to-form UI is explicitly out of scope — a JSON editor with
validation is sufficient for the initial release.

### Multi-select

Uses `inquire::MultiSelect` directly. No wrapper in `jp_inquire` unless a
specific custom behavior is later needed. The answer value is a `Vec<Value>`
of selected option values.

### Progress indicator

Rendered as `[N/M]` before each prompt, where `N` is the 1-indexed position
of the current question and `M` is the total number of questions in the
submitted list. Hidden when `M == 1` (no indicator for single-question
calls, preserving [RFD D27]'s simple UX for the common case).

## Drawbacks

- **Meta-schema expands the LLM surface for getting things wrong.** Duplicate
  `id`s, forward-referencing `when` predicates, `options` on a `text`
  question — all are possible errors. They are caught at submit time and
  returned as structured errors, but the LLM burns tokens correcting them.
- **Back + invalidation adds coordinator state.** The walker needs to track
  collected answers by position, not just by id, so `Back` can discard the
  right tail.
- **Schema answer UX is minimal.** A JSON editor is functional but not
  user-friendly for anything beyond small objects. Iteration is expected.
- **MultiSelect cancel is less uniform** than the inline-select path used
  for Boolean and Select. Esc → cancel menu is the cleanest option but
  requires users to learn one extra binding.

## Alternatives

### LLM-driven branching via multiple `ask_user` calls

The LLM calls `ask_user` with one question, reads the answer, calls again
with the next question tailored to the response. Already possible under
[RFD D27] — no new work needed.

Rejected as the *only* approach because it costs one LLM round-trip per
question. For forms the LLM can predict in advance (most of them), the
round-trips are waste. The `when` predicate collapses them into a single
call.

### Full JSON Schema branching (`oneOf` / discriminators / `$ref`)

Allow the LLM to submit arbitrary JSON Schema with `oneOf` branches, nested
refs, and discriminator-based dispatch.

Rejected: the rendering complexity explodes (generic schema-to-form is a
significant library, not a walker), and LLMs are inconsistent when
constructing `oneOf` schemas. The constrained `when` predicate handles the
common cases at a fraction of the implementation cost.

### Skip the cancel menu; keep Ctrl+C

Keep [RFD D27]'s existing behavior: answer or Ctrl+C.

Rejected: Ctrl+C is a power-user signal, not discoverable. Explicit
`Reply` / `End Turn` / `Back` options in the prompt UI are a meaningful UX
improvement and are cheap to add once every prompt is already a select
(direct for Boolean/Select, pre-prompt for Text/Schema, Esc-driven for
MultiSelect).

### Pre/post-prompt narration

Add optional narrative text before or after each prompt, surfaced as part
of the tool arguments.

Rejected: the LLM can already produce narrative content inline, before or
between tool calls. Post-prompt narration also breaks the "prompt at the
bottom of the terminal" expectation.

## Non-Goals

- **Forward-referencing `when` predicates.** Structural constraint: a
  question's predicate can only reference earlier questions. Prevents cycles
  and keeps the walker O(n).
- **Predicate operators beyond `equals`.** `not_equals`, `one_of`, compound
  predicates (`and`/`or`) are deferred. Add them if real usage demands.
- **Rich form UI for schema-typed answers.** JSON editor only in v1.
- **Timeout / deadline handling.** [RFD 049]'s territory.
- **Nested forms.** A question's `schema` type does not allow embedding
  another `ask_user` call. The LLM can still make a follow-up `ask_user`
  call after the current one returns.

## Risks and Open Questions

- **Meta-schema invalid-submission rate in practice.** Until we have
  telemetry, we don't know how often LLMs produce invalid `when` refs or
  duplicate `id`s. Submit-time validation must return errors clear enough
  for the LLM to self-correct on retry.
- **Schema answer UX iteration.** JSON-editor-plus-validation is the v1
  story, but any real usage will surface sharp edges quickly. Plan for a
  follow-up UX pass if adoption warrants.
- **`MultiSelect` value shape.** Returns a `Vec<Value>` of selected option
  values. If options are heterogeneous (some string, some integer), the
  return is a heterogeneous array. Document the expectation and validate.
- **Single-question call parity with [RFD D27].** A submitted `questions`
  list of length 1 should render identically to the single-question tool
  call in [RFD D27] (no progress indicator, direct prompt). This must be
  verified with snapshot tests.

## Implementation Plan

### Phase 1: Multi-question walker (flat, no branching)

Update `AskUser` to accept `{ questions: [...] }`. Walk the list in order,
collecting answers into a `Map<String, Value>`. Return the map as the tool
result. No `when`, no `Back`, no cancel menu — just the existing
[RFD D27] prompt paths applied N times in sequence.

Validation: unique `id`s; `options`/`schema` required-ness per
`answer_type`.

Depends on [RFD D27] Phase 2. Can be merged independently.

### Phase 2: `when` predicate

Add `when: { question_id, equals }` to the meta-schema. At each question,
evaluate the predicate against collected answers; skip if false. No-forward-
reference validation at submit time. Skipped questions map to `null` in
the return shape.

Depends on Phase 1. Can be merged independently of Phase 3.

### Phase 3: Cancel menu and Back

Add the `Reply` / `End Turn` / `Back` options. Wire Boolean and Select to
extend their inline-select keys (`b`/`r`/`s`). Wire Text and Schema to use
a pre-prompt select. Wire MultiSelect's Esc binding to the cancel menu.

`Back` pops all answers at positions ≥ the back target; re-prompts with
the previous answer pre-populated; re-walks forward.

`Reply` returns `{ cancelled: true, answered: {...partial...} }`.
`End Turn` ends the turn.

Depends on Phase 1.

### Phase 4: `AnswerType::Schema`

Add the `Schema { schema: Value }` variant. Implement the JSON editor
flow: pre-prompt select → JSON editor → validate against schema → accept
or re-prompt on failure.

If [RFD D13] lands first, consume its variant definition instead of
introducing one here.

Depends on Phase 1. Can be merged independently of Phases 2 and 3.

### Phase 5: Multi-select

Integrate `inquire::MultiSelect`. Add `MultiSelect { options }` to
`AnswerType`. Esc → cancel menu.

Depends on Phase 3 (for the cancel menu behavior).

### Phase 6: Progress indicator

Render `[N/M]` before each prompt; hide when `M == 1`.

Depends on Phase 1.

## References

- [RFD D27] — the single-question `ask_user` tool this RFD extends.
- [RFD D13] — defines `AnswerType::Schema` (shared variant).
- [RFD 028] — the inquiry coordinator, walker foundation.
- [RFD 034] — inquiry-specific assistant configuration (relevant for
  schema-typed answers routed through sub-agents, though `ask_user` itself
  stays user-facing).
- [RFD 049] — `exclusive` flag and detached-policy cascade, inherited from
  [RFD D27].

[RFD D27]: D27-built-in-ask_user-tool-for-assistant-initiated-inquiries.md
[RFD D13]: D13-schema-answer-type-and-inherit-model-alias.md
[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 034]: 034-inquiry-specific-assistant-configuration.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
