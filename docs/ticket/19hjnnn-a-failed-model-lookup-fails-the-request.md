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
model metadata (`Provider::model_details`).
When that lookup failed, the entire request failed, even though the returned
metadata (context window, output limit, reasoning support) is optional.

That broke:

- Anthropic-compatible gateways without `/v1/models/{id}`, such as Synthetic's
  `/anthropic` endpoint, on every turn.
- vLLM, llama.cpp, Ollama, Cerebras, Google, and OpenRouter whenever their model
  listing failed, such as behind a router that does not serve `/v1/models`.
- The model picker for unknown OpenAI and Anthropic models, because the lookup
  failed before the stream where the provider error is classified as an unknown
  model.

The lookup also had no timeout, so a slow connection blocked the request
indefinitely.

## Fix

Model lookups are advisory.
If a lookup fails or exceeds the 3-second timeout, JP logs a warning and falls
back to the provider's built-in catalog, or to `ModelDetails::empty` for
unlisted models.

- `crates/jp_llm/src/provider/lookup.rs`: defines `MODEL_LOOKUP_TIMEOUT`,
  `advisory_lookup`, and `find_listed` for providers that resolve metadata from
  their model listing.
- Anthropic: authentication and quota errors still advance the credential chain
  and fail when the chain is exhausted, since the chat request would fail with
  the same credential.
  All other lookup failures fall back to `MODEL_OVERRIDES`, which includes a
  static snapshot of `/v1/models` capabilities (`context_window`,
  `max_output_tokens`, `reasoning`, and `structured_output`).
  When neither configuration nor `ModelDetails` provides an output limit,
  requests default to `max_tokens = 32_000`.
- OpenAI: `model_details` resolves directly from the built-in catalog without
  calling `GET /v1/models/{id}`, which only returns the model ID.

Pre-lookup checks still return errors: credential resolution, OpenAI's ChatGPT
subscription model check, and Claude Code ACP inspection.

Unknown OpenAI and Anthropic models reach the chat stream, where the provider
error is classified as `StreamErrorKind::UnknownModel` so JP lists available
models and offers the interactive model picker.

Found while triaging review feedback on RFD 125.
