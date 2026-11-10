# A failed model lookup fails the request

- **Status**: Done
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-10-27
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: llm-provider=openai
- **Label**: package=jp_cli
- **Label**: package=jp_llm
- **Label**: type=bug

Before every turn, inquiry, and title generation, JP queries the provider for
model metadata via `Provider::model_details`.
Previously, a failed lookup aborted the entire request, even though the metadata
it returns (context window, output token limit, reasoning support) is optional.

This broke:

- Anthropic-compatible gateways that do not serve `/v1/models/{id}`, such as
  Synthetic's `/anthropic` endpoint, on every turn.
- vLLM, llama.cpp, Ollama, Cerebras, Google, and OpenRouter whenever their model
  listing failed (for example, behind a router that does not expose a models
  endpoint).
- The model picker for unknown OpenAI and Anthropic models, because the lookup
  failed before the chat stream could classify the provider refusal as an
  unknown-model error.

Model lookups also had no timeout, so a slow response stalled every request.

## Fix

Model metadata lookups are advisory.
When a lookup fails or exceeds the 3-second timeout, JP logs a warning and falls
back to the provider's built-in table, or to `ModelDetails::empty` for unknown
models.

- `crates/jp_llm/src/provider/lookup.rs`: defines the timeout,
  `advisory_lookup`, and `find_listed` for providers that search their model
  listing.
- Anthropic: a spent or rejected credential still advances the credential chain
  (and fails if the chain is exhausted), because the chat request would fail
  with the same credential error.
  All other lookup failures fall back to `MODEL_OVERRIDES`.
  When reasoning is unconfigured and support is unknown, the API route requests
  summarized adaptive thinking; omitting `thinking` uses Anthropic's default
  (`display: "omitted"` on current models), which bills reasoning tokens without
  returning them.
  The Claude Code subscription route omits `thinking` so Claude Code chooses the
  mode.
- OpenAI: skips the network lookup entirely.
  `GET /v1/models/{id}` returns only the model ID, so `CAPABILITIES` was already
  the sole source of metadata.

Pre-request checks outside the metadata lookup still fail fast: credential
resolution, OpenAI's check that a ChatGPT subscription supports the model, and
Claude Code runtime and login checks on the ACP route.

Lookups no longer fail the turn when a provider does not serve the requested
model, so the chat request reaches the provider and surfaces its refusal.
PR #1298 classifies that refusal as an unknown-model error, lists the served
models, and offers the interactive model picker.

Found while triaging review feedback on RFD 125.
