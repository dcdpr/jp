# Split the Anthropic provider module below 800 lines

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-11
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_llm
- **Label**: type=task

`crates/jp_llm/src/provider/anthropic.rs` has 2,287 production lines, exceeding
the 800-line `[srp] file_length` limit in `.config/rustqual/config.toml`.
It carries a `qual:allow(srp, file_length=2400)` pin so the `qual` gate fails if
the file grows past 2,400 lines.

Split the module into child modules under `provider/anthropic/` by
responsibility:

- `Provider` implementation and credential resolution;
- request construction (`create_request`, cache control, beta features);
- stream and event mapping;
- model catalog (`MODEL_OVERRIDES`) and model mapping;
- error mapping.

Lower the `file_length` pin as each responsibility is extracted, and remove the
suppression once `anthropic.rs` is under 800 production lines.
`qual diff` matches symbol-scoped findings across files by `(rule, symbol)`, so
moving a function does not report its existing findings as new.
