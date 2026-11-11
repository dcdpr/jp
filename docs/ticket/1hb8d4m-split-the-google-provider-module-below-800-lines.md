# Split the Google provider module below 800 lines

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-11
- **Label**: domain=llm
- **Label**: llm-provider=google
- **Label**: package=jp_llm
- **Label**: type=task

`crates/jp_llm/src/provider/google.rs` has 1,129 production lines, exceeding the
800-line `[srp] file_length` limit in `.config/rustqual/config.toml`.
It carries a `qual:allow(srp, file_length=1200)` pin so the `qual` gate fails if
the file grows past 1,200 lines.

Split the module into child modules under `provider/google/` by responsibility:

- `Provider` implementation;
- request construction (`create_request`);
- stream and event mapping;
- model catalog (`MODEL_OVERRIDES`) and model mapping;
- error mapping.

Extracting the model catalog is likely sufficient to bring `google.rs` under 800
production lines, at which point the `file_length` suppression can be removed.
`qual diff` matches symbol-scoped findings across files by `(rule, symbol)`, so
moving a function does not report its existing findings as new.
