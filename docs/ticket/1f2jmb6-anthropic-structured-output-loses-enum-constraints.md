# Anthropic structured output loses `enum` constraints

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-07
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_llm
- **Label**: type=bug

A schema sent to Anthropic for structured output loses its `enum` constraints.
`transform_schema` in `crates/jp_llm/src/provider/anthropic.rs` moves every
keyword it doesn't keep into the property's `description`, and `enum` is one of
them, so the model reads the allowed values as a hint and the response isn't
held to them.

Anthropic's structured outputs support `enum` ([JSON Schema limitations]), and
the function says it mirrors the Python SDK's `transform_schema`, which keeps it
([SDK transform]).

The two also differ on a root `$ref` with `$defs` beside it, the shape
pydantic's `RootModel` writes: the SDK transforms the definitions and keeps
them, and JP returns the `$ref` alone, dropping the definitions it points into.

- [x] Keep a list-valued `enum`, as the SDK does.
- [x] Keep `$defs` and `definitions` beside a root `$ref`.
- [x] Cover both in the transform's tests.

[JSON Schema limitations]: https://docs.claude.com/en/docs/build-with-claude/structured-outputs#json-schema-limitations
[SDK transform]: https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/lib/_parse/_transform.py
