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
- vLLM, llama.cpp, Ollama, Cerebras, Google, and OpenRouter whenever their
  `/v1/models` listing failed (for example, behind a router that does not expose
  that endpoint).
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
  With reasoning support unknown and reasoning unconfigured, an API request
  asks for summarized adaptive thinking instead of taking Anthropic's default,
  which bills reasoning it does not return.
  The Claude Code route still sends nothing and lets Claude Code choose.
- OpenAI: skips the network lookup entirely.
  `GET /v1/models/{id}` returns only the model ID, so `CAPABILITIES` was already
  the sole source of metadata.

Pre-request checks outside the metadata lookup still fail fast: credential
resolution, OpenAI's check that a ChatGPT subscription supports the model, and a
missing Claude Code binary on the ACP route.

A lookup no longer stops the turn for a model the provider does not serve, so
the provider refusal of the chat request comes through instead.
Classifying that refusal as an unknown-model error, listing the available
models, and offering the interactive model picker are #1298's.

Found while triaging review feedback on RFD 125.
