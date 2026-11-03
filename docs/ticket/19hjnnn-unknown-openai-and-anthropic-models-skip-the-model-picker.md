# Unknown OpenAI and Anthropic models skip the model picker

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-10-27
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: llm-provider=openai
- **Label**: package=jp_cli
- **Label**: package=jp_llm
- **Label**: type=bug

When a provider refuses the conversation's model, `jp query` is meant to end
with an "Unknown model" error listing the served models, or offer a picker in an
interactive turn.
For OpenAI's API route and Anthropic's HTTP route, neither happens: the user
gets a generic HTTP error.

`run_turn` looks up the model with `provider.model_details()` before the turn
loop starts (`crates/jp_cli/src/cmd/query.rs`, around line 1205).
Those two providers fetch details directly, so an unknown model fails there:

- OpenAI: `GET /v1/models/{name}` with `error_for_status()`
  (`crates/jp_llm/src/provider/openai.rs`, around line 608).
- Anthropic: `client.models().get(name)`, failing once the credential chain is
  exhausted (`crates/jp_llm/src/provider/anthropic.rs`, around line 465).

The error never reaches the stream, where `StreamErrorKind::UnknownModel` is
classified, so `with_served_models` and `pick_served_model` in
`crates/jp_cli/src/cmd/query/turn_loop.rs` never see it.
The `model_not_found` and `not_found_error` classifications only fire when the
lookup succeeds and the chat request is refused, which is rare for these two
providers.

Providers whose lookup searches the listing and falls back to empty details
(vLLM, llama.cpp, Ollama, Cerebras, Google) are unaffected.

## Fix

Classify the lookup's not-found response as an unknown model in each provider's
`model_details`, and route the lookup error in `run_turn` through the same
served-model handling the turn loop uses, so both the error with its list and
the interactive picker apply.
The picker needs the lookup to be retried with the picked name before the turn
starts.

## Test

A mock provider whose `model_details` returns the unknown-model error: a
non-interactive turn ends with `Error::UnknownModel` carrying the served models,
and an interactive turn with a scripted pick sends its request to the picked
model.

Found while triaging review feedback on RFD 125.
