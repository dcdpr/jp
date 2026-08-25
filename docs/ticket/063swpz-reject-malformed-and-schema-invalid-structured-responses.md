# Reject malformed and schema-invalid structured responses

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-21

The Ollama structured-output cassette returns plain text despite receiving the
title JSON schema.
`EventBuilder` wraps the parse failure in `Value::String` and emits
`ChatResponse::Structured`, so the background title task treats it as structured
data, extracts no title, and returns success without updating the title.

Evidence:

- `crates/jp_llm/tests/fixtures/ollama/test_structured_output.yml`
- `crates/jp_llm/tests/fixtures/ollama/test_structured_output.snap`
- `crates/jp_llm/src/event_builder.rs`
- `crates/jp_task/src/task/title_generator.rs`

Acceptance criteria:

- Malformed JSON cannot be represented as a successful
  `ChatResponse::Structured`.
- Parsed JSON is checked against the schema attached to the request.
- A malformed or nonconforming response produces a typed failure that the retry
  layer or caller can handle.
- Background title generation reports the failure rather than silently
  succeeding without a title.
- Provider fixture tests assert schema conformance, including the existing
  Ollama cassette.
- Cover valid JSON with the wrong shape, wrong field types, and wrong array
  cardinality.
