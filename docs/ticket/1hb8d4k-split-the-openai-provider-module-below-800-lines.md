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
