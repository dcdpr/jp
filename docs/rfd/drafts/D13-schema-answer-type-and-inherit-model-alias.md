# RFD D13: Schema Answer Type and Inherit Model Alias

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-02
- **Extends**: [RFD 028][RFD 028], [RFD 034][RFD 034]
- **Required by**: [RFD D14]
- **Summary**: Adds `AnswerType::Schema` for structured JSON inquiry answers and
  an `inherit` model alias that targets the main assistant model.

## Summary

This RFD adds `AnswerType::Schema`, which lets tools specify arbitrary JSON
schemas for inquiry answers, and a reserved `inherit` model alias, which routes
an inquiry to the main assistant model instead of the inquiry default.

## Motivation

### Schema answer type

The inquiry system supports three answer types: boolean, select, and free-form
text.
These cover simple decisions ("Overwrite?", "Which option?") but cannot express
structured answers with multiple fields in a single response, such as a boolean
decision paired with optional content or a set of configuration values.

Without a structured answer type, a tool must split the interaction into one
sequential inquiry per field, each costing a full LLM round-trip.
A schema-typed answer lets the tool define the expected JSON schema up front and
receive the complete structured response in a single inquiry call.

### Inherit model alias

[RFD 034] introduced per-question model targeting, typically used to route
simple boolean inquiries to a cheaper model (such as Haiku).
Questions that generate file content or write code require the main assistant
model instead, which forces configurations to duplicate `assistant.model.id` in
each per-question override:

```toml
[conversation.tools.my_tool.questions.complex_answer.target]
model.id = "anthropic/claude-sonnet-4-20250514"  # must match assistant.model.id
```

When the user changes their main model, they must update every override that
mirrors it.
A reserved `inherit` alias eliminates this duplication.

## Design

### `AnswerType::Schema`

`AnswerType` in `jp_tool` gains a `Schema` variant for specifying an arbitrary
JSON schema for the inquiry answer, with a matching `InquiryAnswerType::Schema`
variant in `jp_conversation`:

```rust
pub enum AnswerType {
    Boolean,
    Select { options: Vec<String> },
    Text,
    /// Arbitrary JSON schema for the expected answer structure.
    Schema { schema: Map<String, Value> },
}
```

`create_inquiry_schema` inserts the provided schema directly as the `answer`
property:

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

Like other answer types, `Schema` delivers the `answer` value to the tool as a
`serde_json::Value`, and the tool parses the structured fields from the returned
JSON object.

#### User-targeted `Schema` prompts

`Schema` questions target the assistant by default, but interactive TTY sessions
provide a `$EDITOR` fallback when a question routes to the user.
The prompter opens `$EDITOR` with the JSON schema in a comment and an empty JSON
object to fill in, then parses the saved content as JSON.
If the editor returns empty content or invalid JSON, the prompter cancels the
prompt.

### `inherit` model alias

The reserved `inherit` alias routes an inquiry to the parent assistant's model
instead of the inquiry default:

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

Alias validation in `jp_config::model::id::alias` rejects user-defined aliases
named `inherit`.

In `build_inquiry_overrides` (`turn_loop.rs`), a per-question model alias set to
`"inherit"` selects the main assistant's `Arc<dyn Provider>` and `ModelDetails`
(threaded in alongside `default_config`) before the normal alias `resolve()`
path:

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

- **Additional `AnswerType` surface area**: The prompter, inquiry backend, and
  coordinator must each handle `AnswerType::Schema`.
  The wiring is a single match arm per call site, but it expands the surface
  area to maintain.

- **Minimal `$EDITOR` UX for `Schema` prompts**: Writing valid JSON against a
  commented schema in `$EDITOR` is rough, though acceptable because the editor
  prompt is only a fallback for assistant-targeted `Schema` questions.

- **Reserved `inherit` alias name**: Reserving `inherit` is a breaking change
  for any configuration that already defines an alias with that name.

## Alternatives

### Specialized union answer types

Add narrow variants such as `BooleanOrText` (for an overwrite decision paired
with optional content) instead of a general `Schema` variant.
Rejected because each new field combination would require another variant across
`AnswerType`, `InquiryAnswerType`, the prompter, and schema generation, whereas
`Schema` covers arbitrary structured answers with a single variant.

### Duplicate model IDs without `inherit`

Require users to repeat the main model ID or alias on each per-question
override.
While this requires no code changes, changing the main model forces manual
updates across every matching override.
`inherit` prevents this configuration drift with a single check in override
resolution.

### General `inherit` config directive

Support `inherit` across arbitrary config fields (such as system prompts or
sections) rather than only model IDs.
Deferred because model ID inheritance is the only concrete use case, and the
model-specific `"inherit"` alias can be generalized later without breaking
existing configurations.

## Non-Goals

- **Changing existing answer types**: `Boolean`, `Select`, and `Text` remain
  unchanged.
- **Schema validation at the tool layer**: The tool receives a
  `serde_json::Value` and extracts its own fields; validating responses against
  the schema in the inquiry system is out of scope.
- **Inquiry batching**: Combining multiple inquiry questions into a single LLM
  call.

## Risks and Open Questions

- **Provider differences on complex schemas**: Anthropic, OpenAI, and Google all
  support nested objects in structured output, but edge cases around optional
  fields or `oneOf` vary across providers.
  Tools should keep inquiry schemas simple and flat for portability.

- **`inherit` syntax**: A bare word like `inherit` can collide with an existing
  user-defined alias, whereas a sigil-prefixed name such as `$parent` or `@main`
  would avoid collisions at the cost of readability.

## Implementation Plan

### Phase 1: `AnswerType::Schema` and `InquiryAnswerType::Schema`

Add the `Schema` variant to `AnswerType` in `jp_tool` and `InquiryAnswerType` in
`jp_conversation`, wire it through `create_inquiry_schema` and
`tool_question_to_inquiry_question`, and add unit tests for schema generation.
Can be merged independently.

### Phase 2: Prompter support for `Schema`

Implement the `$EDITOR` prompt and JSON parsing for `Schema` questions in
`prompter.rs`.
Depends on Phase 1.

### Phase 3: `inherit` model alias

Reject `inherit` in alias validation, thread the main assistant's provider and
model details into `build_inquiry_overrides` to resolve `"inherit"`, and add
tests for validation and override resolution.
Can be merged independently.

## References

- [RFD 028: Structured Inquiry System for Tool Questions][RFD 028]: the inquiry
  system this RFD extends.
- [RFD 034: Inquiry-Specific Assistant Configuration][RFD 034]: per-question
  model targeting and the three-layer config merge.

[RFD 028]: ../028-structured-inquiry-system-for-tool-questions.md
[RFD 034]: ../034-inquiry-specific-assistant-configuration.md
[RFD D14]: D14-inquiry-driven-file-creation.md
