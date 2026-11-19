# Leave the reasoning level to the model when unset

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: domain=llm
- **Label**: package=jp_config
- **Label**: package=jp_llm
- **Label**: type=enhancement

With `assistant.model.parameters.reasoning` unset, JP selects a level itself:
`ModelDetails::custom_reasoning_config` (`crates/jp_llm/src/model.rs`) resolves
unset and `auto` to `medium` on leveled and budgeted models, and to `high` on
adaptive ones.
Current hosted models reason by default, often above `medium` (Gemini 4 Neon and
Hydrogen default to `high`), so forcing `medium` degrades output quality below
the model's default.
Ollama and llama.cpp go the other way and explicitly disable reasoning when
unset.

What unset sends today:

| Provider                  | Unset reasoning sends                   |
| ------------------------- | --------------------------------------- |
| Anthropic, adaptive       | `effort: high`                          |
| Anthropic, budget-era     | a medium thinking budget                |
| OpenAI, known model       | `effort: medium`                        |
| Google, Gemini 3/4        | `MEDIUM`                                |
| Google, unknown model     | no thinking config; thoughts withheld   |
| OpenRouter, known model   | `effort: medium`                        |
| OpenRouter, unknown model | `effort: minimal, exclude: true`        |
| Cerebras                  | `medium` for gpt-oss; `low` for zai-glm |
| Ollama                    | `think: false`                          |
| llama.cpp                 | `enable_thinking: false` (T-1e5tgqy)    |
| vLLM                      | nothing; the chat template decides      |

## Decided

1. **Unset uses the model's default.** Do not send effort, level, `think`, or
   template kwargs.
   Where an API withholds reasoning content unless requested, request the
   thoughts without specifying a level: Gemini `includeThoughts: true`,
   Anthropic adaptive with `display: summarized`, OpenAI `summary: auto`.
   OpenRouter omits `reasoning` entirely, because `enabled: true` implies
   `medium` there.
2. **`auto` enables reasoning at a model-selected level.** On hosted models that
   always reason, this produces the same request as unset.
   On local models, it sends `think: true` or template kwargs `true`, because a
   chat template's default can be off.
3. **Budget-era models keep a `medium` default.** Omitting `thinking` on
   Anthropic's manual-thinking models disables thinking, and budget-based models
   cannot enable thinking without a token budget.
4. **`off` is unchanged.**

Ollama uses the model's own default when `think` is omitted
(<https://docs.ollama.com/capabilities/thinking>), and the `ollama-rs` fork's
`think` field is already optional.

Fixes T-1e5tgqy and eliminates the warning Google logs when `auto` is clamped to
`high`.
Recording the per-model default in `ReasoningDetails` is tracked in T-1ngtf26.

- [ ] Resolve unset to send no level in `custom_reasoning_config` for leveled
      and adaptive models; keep `medium` for budgeted models.
- [ ] Implement unset and `auto` per provider as specified above.
- [ ] Update the `reasoning` doc comment in `jp_config::model::parameters`.
- [ ] Re-record OpenAI (API and subscription), OpenRouter, Cerebras, Ollama,
      llama.cpp, and Google.
