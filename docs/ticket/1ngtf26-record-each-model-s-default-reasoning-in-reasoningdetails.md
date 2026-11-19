# Record each model's default reasoning in ReasoningDetails

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: domain=llm
- **Label**: package=jp_llm
- **Label**: type=enhancement

`ReasoningDetails` records which reasoning levels a model accepts, but not its
default behavior when a request omits the level (whether it reasons by default,
and at which level).

Several sources report that default:

- Ollama: `thinking.default` from `/api/show`.
- OpenRouter: `default_effort` and `default_enabled` on `/api/v1/models`.
- Hosted catalogs, from the provider docs: Gemini 4 Neon and Hydrogen default to
  `high`, Carbon and Argon to `medium`.

Recording the default lets JP determine whether `auto` (reasoning enabled at a
model-selected level) requires an explicit enable flag for a given model, and
lets JP display the effective reasoning level for a request.

- [ ] Add the default reasoning level to `ReasoningDetails`.
- [ ] Populate it from Ollama, OpenRouter, and the hosted catalogs.
- [ ] Use it when a provider determines whether `auto` requires an explicit
      enable flag.
