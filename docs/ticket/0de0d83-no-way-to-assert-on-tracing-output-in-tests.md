# No way to assert on tracing output in tests

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-09-04

Nothing in the workspace can assert that a `tracing` event fired.
Grepping `tracing_subscriber|with_default|logs_contain` across `jp_llm` and
`jp_test` returns no test helper, so a log line is untestable and a deleted one
is invisible.

That is a gap wherever a log line *is* the behaviour.
PR \#1083 added a `warn!` in `jp_llm::provider::openai_compat::parse_chunk` that
fires when a stream chunk is discarded — the whole point of the change, since
the alternative is diagnosing a silent drop from a blank retry loop.
Its tests pin `parse_chunk`'s return value and the `error` field's round-trip,
both of which stay green if the `warn!` is deleted.

`test-log` is already a dev-dependency in `jp_llm`, so the subscriber plumbing
is half present.
What is missing is a capture layer plus an assertion helper, somewhere reusable
— `jp_test` is the natural home given it already carries `mock` and `macros`.

## The decision the fix forces

Whether the helper asserts on rendered text or on structured fields:

- **Rendered text** reads naturally in a test and catches wording changes, but
  breaks on every rewording and cannot distinguish two events that render alike.
- **Level plus fields** is stabler and matches how the events are actually
  written, but a test then says nothing about whether the message is
  intelligible.

The `parse_chunk` case wants the level and the `provider` field, not the
sentence.
Worth settling before writing the helper, since it decides whether captured
events are stored as strings or as field maps.

Also needs deciding: whether capture is per-test (a scoped subscriber, safe
under `cargo test`'s thread-per-test) or global.
Per-test is the only one that works with parallel execution, and it constrains
the API to something the test holds rather than a free function.
