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

- [ ] Add the default enabled state (on, off, or unknown) and the default level
      to `ReasoningDetails`.
      OpenRouter's `default_effort` is the level used when reasoning is enabled,
      not evidence that reasoning is on by default.
- [ ] Populate both from Ollama, OpenRouter (making `default_enabled` optional
      in `jp_openrouter`), and the hosted catalogs.
- [ ] Use the default enabled state when a provider determines whether `auto`
      requires an explicit enable flag.
