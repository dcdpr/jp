# Anthropic structured output loses `enum` constraints

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-07
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_llm
- **Label**: type=bug

`transform_schema` in `crates/jp_llm/src/provider/anthropic.rs` strips
unsupported JSON Schema keywords and appends them to the property's
`description`.
Because `transform_schema` does not preserve `enum`, allowed values are moved
into `description` as a hint rather than enforced as a schema constraint.

Anthropic's structured outputs support `enum` ([JSON Schema limitations]), and
Anthropic's Python SDK preserves array-valued `enum` keywords in its
`transform_schema` implementation ([SDK transform]).

`transform_schema` also returns early on `$ref` before extracting `$defs` and
`definitions`.
On a schema whose root is a `$ref` with sibling `$defs` (such as Pydantic's
`RootModel`), the early return drops the definitions that the root `$ref`
targets.

- [x] Preserve array-valued `enum` keywords, matching the Python SDK.
- [x] Extract `$defs` and `definitions` before returning early on a root `$ref`.
- [x] Cover both cases in `transform_schema` unit tests.

[JSON Schema limitations]: https://docs.claude.com/en/docs/build-with-claude/structured-outputs#json-schema-limitations
[SDK transform]: https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/lib/_parse/_transform.py
