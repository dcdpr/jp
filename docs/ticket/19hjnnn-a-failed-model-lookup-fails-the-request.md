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

Before every turn, inquiry, and title, JP asks the provider about the model
(`Provider::model_details`).
A lookup that failed failed the request, though what it reports (context window,
output limit, reasoning support) only fills in details the request can go ahead
without.

That broke:

- Anthropic-compatible gateways without `/v1/models/{id}`, such as Synthetic's
  `/anthropic` endpoint, on every turn.
- vLLM, llama.cpp, Ollama, Cerebras, Google, and OpenRouter whenever their
  listing failed, such as behind a router that serves no `/v1/models`.
- The model picker for unknown OpenAI and Anthropic models: their lookup failed
  before the stream, where the refusal is classified as an unknown model.

A lookup on a slow connection also held up every request, with no timeout.

## Fix

The lookup is advisory.
A failed lookup, or one still running after three seconds, is logged at `warn`,
and the provider falls back to its built-in table, or to empty details for a
model the table does not know.

- `crates/jp_llm/src/provider/lookup.rs`: the timeout, `advisory_lookup`, and
  `find_listed` for providers that search their listing.
- Anthropic: a spent or refused credential still moves the chain on, and an
  exhausted chain still fails, since the request would be refused too.
  Every other failure falls back to the override table.
- OpenAI: no request at all.
  `GET /v1/models/{id}` reports only the id, so the capability table was already
  the only source.

Errors from outside the lookup request still fail: credential resolution,
OpenAI's check that a ChatGPT subscription serves the model, and a missing
Claude Code on the ACP route.

An unknown model now reaches the stream, where the refusal is classified, so the
error with the served models, and the picker, apply.

Found while triaging review feedback on RFD 125.
