# RFD D13: Schema Answer Type and Inherit Model Alias

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-02
- **Extends**: [RFD 028][RFD 028], [RFD 034][RFD 034]

## Summary

This RFD adds two building blocks to the inquiry system: an
`AnswerType::Schema` variant that lets tools specify arbitrary JSON schemas for
inquiry answers, and a reserved `inherit` model alias that routes an inquiry to
the main assistant model instead of the (typically cheaper) inquiry default.

## Motivation

### Schema answer type

Tools can currently ask three kinds of questions via the inquiry system:
boolean, select, and free-form text. These cover simple decisions
("Overwrite?", "Which option?") but cannot express structured answers where
multiple fields are needed in a single response — for example, a boolean
decision paired with optional content, or a set of configuration values.

Today, a tool needing structured input must split the interaction into multiple
sequential inquiries (one per field), each costing a full LLM round-trip. A
schema-typed answer lets the tool describe the expected structure in one JSON
schema and receive a complete structured answer in a single inquiry call.

### Inherit model alias

[RFD 034] introduced per-question model targeting so that different inquiry
questions can be routed to different models. The typical setup routes inquiries
to a cheap model (e.g. Haiku) to save cost on simple boolean questions.

Some inquiry questions require the main assistant model's capabilities — for
example, generating file content or writing code. Today, the only way to route
such a question to the main model is to duplicate the model ID in the
per-question config:

```toml
[conversation.tools.my_tool.questions.complex_answer.target]
model.id = "anthropic/claude-sonnet-4-20250514"  # must match assistant.model.id
```

If the user changes their main model, they must remember to update every
per-question override that mirrors it. A reserved `inherit` alias eliminates
this duplication.

## Design

### `AnswerType::Schema`

A new variant on `AnswerType` (in `jp_tool`) allows tools to specify an
arbitrary JSON schema for the expected inquiry answer:

```rust
pub enum AnswerType {
    Boolean,
    Select { options: Vec<String> },
    Text,
    /// Arbitrary JSON schema for the expected answer structure.
    Schema { schema: Map<String, Value> },
}
```

A matching `InquiryAnswerType::Schema` variant is added to `jp_conversation`.

In the inquiry system (`create_inquiry_schema`), the `Schema` variant uses the
provided schema directly as the `answer` property:

```rust
AnswerType::Schema { schema } => Value::Object(schema.clone()),
```

This produces a structured output request like:

```json
{
  "type": "object",
  "properties": {
    "answer": {
      "type": "object",
      "properties": {
        "overwrite": { "type": "boolean" },
        "content": { "type": "string" }
      },
      "required": ["overwrite"],
      "additionalProperties": false
    }
  },
  "required": ["answer"],
  "additionalProperties": false
}
```

The tool receives the `answer` value as a `serde_json::Value` — the same as
other answer types. The tool is responsible for parsing the structured fields
from the returned JSON object.

#### User-targeted Schema prompts

When a `Schema`-typed question is routed to the user (interactive TTY), the
prompter opens `$EDITOR` with a template containing the JSON schema as a
comment and an empty JSON object for the user to fill in. The edited content is
parsed as JSON. If parsing fails or the editor returns empty content, the prompt
is treated as cancelled.

This is a power-user escape hatch. In practice, `Schema` questions are expected
to be routed to the assistant.

### `inherit` model alias

A reserved alias `inherit` tells the inquiry system to use the parent
assistant's model instead of the inquiry default:

```toml
[conversation.tools.my_tool.questions.complex_answer.target]
model.id = "inherit"
```

Resolution order with `inherit`:

```
per-question target = "inherit"
  → resolves to main assistant model (skips inquiry default)
```

#### Implementation

`inherit` is a reserved alias name. User-defined aliases named `inherit` are
rejected during alias validation in `jp_config::model::id::alias`.

Resolution happens in `build_inquiry_overrides` (`turn_loop.rs`). When the
per-question model alias is `"inherit"`, the function uses the main assistant's
provider and model details instead of looking up the alias map. This requires
threading the main model's `Arc<dyn Provider>` and `ModelDetails` into
`build_inquiry_overrides`, alongside the existing `default_config` parameter.

The check is a string comparison on the alias value before the normal
`resolve()` path:

```rust
let is_inherit = matches!(&per_q.model.id, PartialModelIdOrAliasConfig::Alias(a) if a == "inherit");

let (inq_provider, inq_model) = if is_inherit {
    (Arc::clone(&main_provider), main_model.clone())
} else if has_model_override {
    // existing resolution logic
    ...
} else {
    (Arc::clone(&default_config.provider), default_config.model.clone())
};
```

## Drawbacks

- **Schema answer type adds complexity to the inquiry system.** `AnswerType`
  gains a fourth variant that the prompter, inquiry backend, and coordinator
  must handle. The wiring is mechanical (one new arm in each match), but it's
  still more surface area to maintain.

- **`$EDITOR` UX for Schema prompts is bare-bones.** Presenting a JSON schema
  in an editor and expecting the user to produce valid JSON is not a polished
  experience. This is acceptable because Schema questions are expected to target
  the assistant, not the user. The editor path is a fallback, not the primary
  flow.

- **`inherit` occupies a reserved word.** If a user has an existing alias named
  `inherit`, this is a breaking change. Unlikely in practice but worth noting.

## Alternatives

### Specialized union answer types

Instead of a general `Schema`, add specific types like `BooleanOrText` for the
known use case (overwrite decision + optional content).

Rejected: too narrow. Each new combination would require a new variant in
`AnswerType`, `InquiryAnswerType`, the prompter, and schema generation. `Schema`
is a single general-purpose extension that covers all structured answer needs.

### No `inherit` — users duplicate model IDs

Users set the same model alias explicitly on per-question overrides. No code
changes needed.

Rejected: fragile. When the user changes their main model, every per-question
override that mirrors it must be updated manually. `inherit` is a one-line
addition to the resolution logic that eliminates a class of configuration drift.

### `inherit` as a general config directive

Make `inherit` work for any config field (system prompt, sections, etc.), not
just model IDs.

Deferred: the only current need is model ID inheritance. Generalizing adds
complexity without a concrete use case. If needed later, the model-only
`inherit` is forward-compatible — it can be extended without breaking existing
configs.

## Non-Goals

- **Changing existing answer types.** Boolean, Select, and Text remain
  unchanged.
- **Schema validation at the tool layer.** The tool receives a
  `serde_json::Value` and is responsible for extracting fields. Adding
  schema validation to the inquiry system is orthogonal.
- **Inquiry batching.** Combining multiple inquiry questions into a single LLM
  call.

## Risks and Open Questions

- **Structured output support for nested schemas.** The `Schema` answer type
  enables arbitrarily complex JSON schemas. All major providers (Anthropic,
  OpenAI, Google) support nested objects in structured output, but edge cases
  with optional fields or `oneOf` may behave differently across providers.
  Tools should stick to simple, flat-ish schemas for reliability.

- **`inherit` alias naming.** The name is clear and unlikely to collide, but it
  is technically a breaking change for anyone who defined an alias called
  `inherit`. An alternative like `$parent` or `@main` would be more visually
  distinct but less readable.

## Implementation Plan

### Phase 1: `AnswerType::Schema` and `InquiryAnswerType::Schema`

Add the `Schema` variant to `AnswerType` in `jp_tool` and `InquiryAnswerType`
in `jp_conversation`. Wire it through `create_inquiry_schema` in the inquiry
system and `tool_question_to_inquiry_question` in the coordinator. Add unit
tests for schema generation.

Can be merged independently.

### Phase 2: Prompter support for `Schema`

Add the `$EDITOR`-based prompt for `Schema`-typed questions in `prompter.rs`.
The editor opens with a JSON template showing the schema; the user fills in
values and saves. Parse the result as JSON.

Depends on Phase 1. Can be merged independently.

### Phase 3: `inherit` model alias

Add the reserved `inherit` alias. Reject it in alias validation. Handle it in
`build_inquiry_overrides` by threading the main assistant's provider and model
details through. Add tests for alias validation and override resolution.

Can be merged independently.

## References

- [RFD 028: Structured Inquiry System for Tool Questions][RFD 028] — the
  inquiry system this extends.
- [RFD 034: Inquiry-Specific Assistant Configuration][RFD 034] — per-question
  model targeting and the three-layer config merge.

[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 034]: 034-inquiry-specific-assistant-configuration.md
