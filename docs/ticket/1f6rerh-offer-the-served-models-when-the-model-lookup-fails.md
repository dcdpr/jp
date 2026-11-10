# Offer the served models when the model lookup fails

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-07
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: llm-provider=openai
- **Label**: package=jp_cli
- **Label**: type=follow-up

`jp query` looks up the conversation's model (`provider.model_details`) in
`Query::run_turn` before the turn loop starts.
Anthropic and OpenAI (on its API route) query a per-model endpoint, so an
unknown model fails during that lookup.
The failure never reaches the model picker added in PR #1298 for refused stream
requests, and the error lists no served models.

The `unknown_model_details` recordings in `jp_llm` pin which providers fail the
lookup.

Recovery here is a different flow from the turn loop's: nothing has been
appended yet, so the picker would run before `TurnStart`, record the pick as an
ordinary config delta, and look up the picked model again.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-10T21:00:00Z

Resolved by T-19hjnnn (PR #1299), which makes `Provider::model_details` advisory
across providers and removes OpenAI's network lookup.
Unknown Anthropic and OpenAI models return `ModelDetails::empty` from
`model_details` and fail on the chat request instead, reaching PR #1298's
unknown-model classification, served-models error, and interactive model picker
without a separate pre-`TurnStart` recovery flow.
