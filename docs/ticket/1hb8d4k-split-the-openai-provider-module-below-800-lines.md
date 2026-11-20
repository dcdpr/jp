# Split the OpenAI provider module below 800 lines

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-11
- **Label**: domain=llm
- **Label**: llm-provider=openai
- **Label**: package=jp_llm
- **Label**: type=task

`crates/jp_llm/src/provider/openai.rs` has 2,754 production lines, exceeding the
800-line `[srp] file_length` limit in `.config/rustqual/config.toml`.
It carries a `qual:allow(srp, file_length=2900)` pin so the `qual` gate fails if
the file grows past 2,900 lines.

Split the module into child modules under `provider/openai/` by responsibility:

- `Provider` implementation, credential resolution, and usage-reset redemption;
- request construction (`create_request`, subscription request rewrite);
- stream and event mapping;
- model catalog (`MODEL_OVERRIDES`, ~900 lines) and model mapping;
- error mapping.

Lower the `file_length` pin as each responsibility is extracted, and remove the
suppression once `openai.rs` is under 800 production lines.
A function that moves to a child module keeps the baseline of its existing
findings: `qual diff` follows an item once the old file no longer defines it and
the new file didn't define it at the base commit.
Move a function whole.
A function left behind under its old name, or moved into a file that already had
an item of that name, has its findings reported as new.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-17T21:55:19Z

First step done: `MODEL_OVERRIDES`, `map_model`, `is_api_only`,
`subscription_models`, and `model_overrides` moved unchanged to
`provider/openai/models.rs`.
The pin on `openai.rs` is lowered to `file_length=1900`.

`models.rs` is 1,022 production lines, nearly all of it the static table, so it
trips `SRP-002` on its own.
A split along the table's own grain would be current versus retired models (the
entries carrying `ModelDeprecation::deprecated`), but that is a judgement call;
see the conversation that moved it.
