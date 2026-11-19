# Leave the reasoning level to the model when unset

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: domain=llm
- **Label**: package=jp_config
- **Label**: package=jp_llm
- **Label**: type=enhancement

With `assistant.model.parameters.reasoning` unset, JP picks a level itself:
`ModelDetails::custom_reasoning_config` (`crates/jp_llm/src/model.rs`) resolves
unset and `auto` to `medium` on leveled and budgetted models, and to `high` on
adaptive ones.
Current hosted models reason by default, often above `medium` (Gemini 4 Neon and
Hydrogen default to `high`), so this leaves quality on the table rather than
recovering it.
Ollama and llama.cpp go the other way and send an explicit "off".

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

1. **Unset means the model's default.** No effort, level, `think`, or template
   kwargs are sent.
   Where an API withholds reasoning unless asked, JP still asks to see it
   without naming a level: Gemini `includeThoughts: true`, Anthropic adaptive
   with `display: summarized`, OpenAI `summary: auto`.
   OpenRouter omits `reasoning` entirely, since `enabled: true` means `medium`
   there.
2. **`auto` means reasoning on, at a level the model picks.** On hosted models
   that always reason this is the same request as unset.
   On local models it is `think: true` or kwargs `true`, because a template's
   default can be off.
3. **Budget-era models keep a medium default.** Omitting `thinking` on
   Anthropic's manual-thinking models turns thinking off, and a budget model has
   no way to say "on" without a number.
4. **`off` is unchanged.**

Ollama uses the model's own default when `think` is omitted
(<https://docs.ollama.com/capabilities/thinking>), and the `ollama-rs` fork's
`think` field is already optional.

Fixes T-1e5tgqy, and the warning Google logs when `auto` is clamped to `high`.
The per-model default itself is T-1ngtf26.

- [ ] Resolve unset to "send nothing" in `custom_reasoning_config` for leveled
      and adaptive models; keep `medium` for budgetted ones.
- [ ] Express unset and `auto` per provider as above.
- [ ] Update the `reasoning` doc comment in `jp_config::model::parameters`.
- [ ] Re-record OpenAI (API and subscription), OpenRouter, Cerebras, Ollama,
      llama.cpp, and Google.
