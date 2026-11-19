# Record each model's default reasoning in ReasoningDetails

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: domain=llm
- **Label**: package=jp_llm
- **Label**: type=enhancement

`ReasoningDetails` says which reasoning levels a model accepts, but not what it
does when a request names no level: whether it reasons at all, and at which
level.

Several sources report that default:

- Ollama: `thinking.default` from `/api/show`.
- OpenRouter: `default_effort` and `default_enabled` on `/api/v1/models`.
- Hosted catalogs, from the provider docs: Gemini 4 Neon and Hydrogen default to
  `high`, Carbon and Argon to `medium`.

With the default recorded, JP knows whether `auto` (reasoning on, the model
picks the level) needs an explicit "on" for a given model, and can show the user
the level a request actually ran at.

- [ ] Add the default to `ReasoningDetails`.
- [ ] Fill it from Ollama, OpenRouter, and the hosted catalogs.
- [ ] Use it where a provider decides whether `auto` needs an explicit enable.
