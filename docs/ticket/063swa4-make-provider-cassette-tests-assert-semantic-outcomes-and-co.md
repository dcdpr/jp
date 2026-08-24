# Make provider cassette tests assert semantic outcomes and consumed interactions

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-08-21

The provider VCR tests snapshot whatever happened but usually have no semantic
assertion.
`Vcr::cassette` also discards the `MockSet` returned by `playback_async`, so
recorded interactions are not checked for exactly one use.
Optional tool follow-ups can be silently skipped and leave stale cassette
entries without failing.

Current green examples include empty llama.cpp model details, malformed Ollama
structured output, invalid tool arguments, and empty Google post-tool responses.

Acceptance criteria:

- Assert every cassette interaction is consumed exactly once during playback.
- Fail on unexpected requests and unused recorded responses.
- Require request-kind invariants: forced tool name, tool argument conformance,
  structured schema conformance, nonempty or explicitly empty post-tool outcome,
  requested model ID, and nonempty model lists.
- Remove no-op assertion defaults where the request kind has a meaningful
  contract.
- Make skipped `ToolCallResponse` requests explicit in the test declaration
  rather than controlled by a boolean that silently returns `None`.
- Add a harness regression test with an intentionally unused second cassette
  interaction.
