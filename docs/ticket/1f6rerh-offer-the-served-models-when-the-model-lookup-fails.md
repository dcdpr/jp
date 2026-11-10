# Offer the served models when the model lookup fails

- **Status**: Todo
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

- [ ] Classify a refused lookup as an unknown model for Anthropic and OpenAI
- [ ] In an interactive query, offer the provider's served models and continue
      with the pick
- [ ] Non-interactively, end with "Unknown model" listing the served models
- [ ] A test that starts above `run_turn_loop`
