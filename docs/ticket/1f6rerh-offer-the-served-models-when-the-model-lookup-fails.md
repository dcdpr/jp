# Offer the served models when the model lookup fails

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-07
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: llm-provider=openai
- **Label**: package=jp_cli
- **Label**: type=follow-up

`jp query` looks the conversation's model up (`provider.model_details`) in
`Query::run_turn` before the turn loop starts.
Anthropic, and OpenAI on its API route, ask the API about the named model, so an
unknown model fails there with the lookup error.
It never reaches the picker PR #1298 added for a refused request, and the error
lists no served models.

The `unknown_model_details` recordings in `jp_llm` pin which providers fail the
lookup.

Recovery here is a different flow from the turn loop's: nothing has been
appended yet, so the picker would run before `TurnStart`, record the pick as an
ordinary config delta, and look the picked model up again.

- [ ] Classify a refused lookup as an unknown model for Anthropic and OpenAI
- [ ] In an interactive query, offer the provider's served models and continue
      with the pick
- [ ] Non-interactively, end with "Unknown model" listing the served models
- [ ] A test that starts above `run_turn_loop`
