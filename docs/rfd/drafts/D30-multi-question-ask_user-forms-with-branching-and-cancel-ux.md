# RFD D30: Multi-Question ask_user Forms with Branching and Cancel UX

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-17
- **Extends**: [RFD 083]
- **Summary**: Extends `ask_user` to collect multi-question forms with
  predicate-gated branching and `Reply` / `End Turn` / `Back` cancellation
  controls.

## Summary

This RFD extends the built-in `ask_user` tool ([RFD 083]) to collect
multi-question forms in a single tool call.
It adds predicate-gated branching, `Reply` / `End Turn` / `Back` navigation, and
`boolean`, `select`, `multi_select`, `text`, and `schema` answer types.
The assistant supplies a question graph in the tool call arguments, and the
coordinator walks it to prompt the user and return all collected answers at
once.

## Motivation

[RFD 083] makes `ask_user` a single-question tool.
In practice, user-facing flows often need several related answers (e.g. "which
directory, and with what permissions?").
With a single-question tool, the LLM must make a separate `ask_user` call for
each answer, paying an LLM round-trip per question.
When the questions and follow-ups are predictable, those extra round-trips are
wasted because the LLM already knows what to ask.

A single `ask_user` call with multiple questions and predicate-gated follow-ups
collapses those round-trips into one.
Combined with typed-schema answers ([RFD D13]), it also supports lightweight
user-facing forms (e.g. "give me the structured config for this migration")
without requiring custom tool prompts.

[RFD 083] also provides only two ways out of a prompt: answer or press Ctrl+C.
Explicit `Reply`, `End Turn`, and `Back` options make cancellation and
navigation discoverable, including on single-question `ask_user` calls.

## Design

### User-visible behavior

The assistant submits a list of questions.
The user answers them in order and can go back, cancel with a reply, or end the
turn at any prompt:

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
(`[1/3]`, `[2/3]`, …).
Questions whose `when` predicate evaluates to `false` are skipped silently.

### Tool arguments meta-schema

The `ask_user` argument schema defines the question list the LLM can submit.
A single-question call from [RFD 083] is a `questions` array with one entry:

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

Constraining the meta-schema keeps branching predictable: the LLM populates a
fixed question structure rather than generating arbitrary JSON Schema.
This avoids the rendering complexity of generic `oneOf` and discriminator flows
while covering common conditional follow-ups.

At submit time, the tool validates the question list and returns a structured
error to the LLM if any check fails:

- `id` values must be unique.
- `options` is required if and only if `answer_type` is `select` or
  `multi_select`.
- `schema` is required if and only if `answer_type` is `schema`.
- `when.question_id` must reference an `id` earlier in the list.
  Disallowing forward references prevents cycles by construction.

### Branching: the `when` predicate

Each question accepts an optional `when` predicate:

```jsonc
{ "question_id": "<earlier id>", "equals": <value> }
```

When the walker reaches a question, it evaluates `when` against the answers
collected so far.
If the predicate is absent or evaluates to `true`, the coordinator prompts the
user.
If it evaluates to `false`, the walker skips the question and records `null` for
its ID in the return map.

Version 1 supports only `equals`.
Additional operators (`not_equals`, `one_of`, nested predicates) are deferred to
follow-up RFDs if needed.

### Cancel UX

Every prompt offers three cancel paths alongside the answer:

| Option     | Effect                                                      |
| ---------- | ----------------------------------------------------------- |
| `Reply`    | Cancel the form and return a cancel marker to the LLM       |
|            | without ending the turn.                                    |
| `End Turn` | End the turn immediately, matching Ctrl+C with an explicit, |
|            | discoverable option.                                        |
| `Back`     | Re-prompt the previous *answered* question. Omitted when    |
|            | there is no prior answered question.                        |

How these options are presented depends on the answer type:

- **Boolean**: inline-select extends the existing git-style keys (`y` / `Y` /
  `n` / `N`) with `b` / `r` / `s`.
- **Select**: inline-select appends `b` / `r` / `s` to the option keys.
- **MultiSelect**: `inquire::MultiSelect` submits on Enter; Esc opens a `[Back /
  Reply / End Turn]` cancel menu.
- **Text**: a pre-prompt select `[Answer / Back / Reply / End Turn]` runs first,
  and `Answer` opens the text input.
- **Schema**: uses the same pre-prompt select as Text; `Answer` opens a JSON
  editor and validates the input against the question's schema on submit.

### Back navigation and answer invalidation

`Back` re-prompts the previous answered question with its current answer
pre-populated.
When the user submits a new answer, the walker **discards all answers collected
after that position** and walks forward again.
Re-evaluating subsequent `when` predicates against the updated answer
automatically switches branches when a condition changes.

### Return shape

The tool returns a JSON object mapping each `question_id` to its answer.
Questions that were skipped, or invalidated by `Back` and not re-answered, map
to `null`:

```jsonc
{
  "apply": true,
  "env": "production",
  "note": null
}
```

Every submitted question ID is present in the returned object, giving the LLM a
uniform, parseable response where `null` marks unanswered questions.

### Cancel return shape

On `Reply`, the tool returns a structured cancel marker containing the answers
collected before cancellation, so the LLM can proceed with partial information
or ask again:

```jsonc
{
  "cancelled": true,
  "answered": {
    "apply": true
  }
}
```

On `End Turn`, the turn ends before the tool response is delivered, matching
Ctrl+C.

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

The variant can land independently; if [RFD D13] (which defines the same variant
for tool-authored questions) lands first, this RFD reuses that definition.

The v1 renderer opens a JSON editor pre-populated with the question's `default`
(if any) and validates the input against `schema` on submit.
Rich schema-to-form UI is out of scope for the initial release.

### Multi-select

`MultiSelect` uses `inquire::MultiSelect` directly without a `jp_inquire`
wrapper, unless custom behavior is later needed.
The answer value is a `Vec<Value>` of selected option values.

### Progress indicator

Each prompt is prefixed with `[N/M]`, where `N` is the 1-indexed position of the
current question and `M` is the total number of submitted questions.
The indicator is hidden when `M == 1`, preserving the [RFD 083] single-question
prompt.

## Drawbacks

- **Larger LLM error surface.** The LLM can submit duplicate `id`s,
  forward-referencing `when` predicates, or `options` on a `text` question.
  Submit-time validation catches these and returns structured errors, but the
  LLM spends tokens correcting them.
- **Additional coordinator state for `Back`.** The walker must track collected
  answers by position rather than only by ID so `Back` can discard subsequent
  answers.
- **Minimal `schema` answer UX.** A JSON editor works for small objects but is
  awkward for larger schemas and will likely need iteration.
- **Less uniform `MultiSelect` cancellation.** Unlike Boolean and Select, which
  bind cancel actions to inline keys, `MultiSelect` requires pressing Esc to
  open the cancel menu.

## Alternatives

### LLM-driven branching via multiple `ask_user` calls

Under [RFD 083], the LLM can already call `ask_user` one question at a time and
choose the next question from the previous answer.
Relying on this alone costs one LLM round-trip per question.
When the LLM can predict the follow-up questions in advance, `when` predicates
collapse the exchange into a single call.

### Full JSON Schema branching (`oneOf` / discriminators / `$ref`)

Allowing the LLM to submit arbitrary JSON Schema with `oneOf` branches, nested
`$ref`s, and discriminator-based dispatch would turn the walker into a general
schema-to-form library, and LLMs construct `oneOf` schemas unreliably.
The constrained `when` predicate covers common conditional flows at a fraction
of the implementation cost.

### Skip the cancel menu; keep Ctrl+C

Keeping [RFD 083]'s answer-or-Ctrl+C behavior avoids new prompt options, but
Ctrl+C is not discoverable and cannot express `Reply` or `Back`.
Because every prompt already uses a selection UI (inline keys for Boolean and
Select, a pre-prompt menu for Text and Schema, and an Esc menu for MultiSelect),
surfacing `Reply`, `End Turn`, and `Back` adds little implementation cost.

### Pre/post-prompt narration

Adding optional narrative text before or after each prompt in the tool arguments
duplicates what the LLM can already write inline before or between tool calls.
Post-prompt narration also breaks the expectation that the active prompt sits at
the bottom of the terminal.

## Non-Goals

- **Forward-referencing `when` predicates.** A question's predicate can only
  reference earlier questions, which prevents cycles and keeps the walker O(n).
- **Predicate operators beyond `equals`.** Operators such as `not_equals`,
  `one_of`, and compound `and`/`or` predicates are deferred until real usage
  requires them.
- **Rich form UI for schema-typed answers.** Version 1 uses a JSON editor.
- **Timeout / deadline handling.** Covered by [RFD 049].
- **Nested forms.** A `schema` question cannot embed another `ask_user` call.
  The LLM can issue a follow-up `ask_user` call after the current one returns.

## Risks and Open Questions

- **Invalid submission rate.** How often LLMs produce invalid `when` references
  or duplicate `id`s is unknown until we have telemetry.
  Submit-time validation errors must be clear enough for the LLM to self-correct
  on retry.
- **Schema answer UX iteration.** A JSON editor with validation is sufficient
  for v1, but real usage will surface sharp edges that need a follow-up UX pass.
- **`MultiSelect` value shape.** `MultiSelect` returns a `Vec<Value>` of
  selected option values, so heterogeneous options (mixing strings and integers)
  produce a heterogeneous array.
  This behavior must be documented and validated.
- **Single-question call parity with [RFD 083].** A `questions` list of length 1
  must render identically to a single-question call in [RFD 083] (direct prompt,
  no progress indicator), verified with snapshot tests.

## Implementation Plan

### Phase 1: Multi-question walker (flat, no branching)

Update `AskUser` to accept `{ questions: [...] }`, walk the list in order using
the existing [RFD 083] prompt paths, and return the collected answers as a
`Map<String, Value>`.
Validate that `id` values are unique and that `options` and `schema` are present
when required by `answer_type`.
Branching (`when`), `Back`, and the cancel menu are omitted in this phase.

Depends on [RFD 083] Phase 2 and can be merged independently.

### Phase 2: `when` predicate

Add `when: { question_id, equals }` to the meta-schema and reject forward
references at submit time.
At each question, evaluate the predicate against the collected answers; skip the
question and record `null` in the return shape when it evaluates to `false`.

Depends on Phase 1 and can be merged independently of Phase 3.

### Phase 3: Cancel menu and Back

Add the `Reply` / `End Turn` / `Back` options:

- Extend Boolean and Select inline-select keys with `b` / `r` / `s`.
- Add the pre-prompt select before Text and Schema prompts.
- Bind Esc in MultiSelect to the cancel menu.

`Back` pops all answers at positions ≥ the back target, re-prompts with the
previous answer pre-populated, and re-walks forward.
`Reply` returns `{ cancelled: true, answered: {...partial...} }`, and `End Turn`
ends the turn.

Depends on Phase 1.

### Phase 4: `AnswerType::Schema`

Add the `Schema { schema: Value }` variant (or consume the definition from [RFD
D13] if it lands first).
Implement the JSON editor flow: show the pre-prompt select, open the JSON
editor, validate against the schema, and accept the value or re-prompt on
failure.

Depends on Phase 1 and can be merged independently of Phases 2 and 3.

### Phase 5: Multi-select

Add `MultiSelect { options }` to `AnswerType`, integrate `inquire::MultiSelect`,
and bind Esc to the cancel menu.

Depends on Phase 3 (for the cancel menu behavior).

### Phase 6: Progress indicator

Render `[N/M]` before each prompt and hide it when `M == 1`.

Depends on Phase 1.

## References

- [RFD 083]: the single-question `ask_user` tool this RFD extends.
- [RFD D13]: defines the shared `AnswerType::Schema` variant.
- [RFD 028]: the inquiry coordinator that the question walker builds on.
- [RFD 034]: inquiry-specific assistant configuration (relevant for schema-typed
  answers routed through sub-agents, though `ask_user` itself stays
  user-facing).
- [RFD 049]: `exclusive` flag and detached-policy cascade, inherited from [RFD
  083].

[RFD 028]: ../028-structured-inquiry-system-for-tool-questions.md
[RFD 034]: ../034-inquiry-specific-assistant-configuration.md
[RFD 049]: ../049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 083]: ../083-built-in-ask_user-tool-for-assistant-initiated-inquiries.md
[RFD D13]: D13-schema-answer-type-and-inherit-model-alias.md
