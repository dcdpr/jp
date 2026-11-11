# Split the Google provider module below 800 lines

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-11
- **Label**: domain=llm
- **Label**: llm-provider=google
- **Label**: package=jp_llm
- **Label**: type=task

`crates/jp_llm/src/provider/google.rs` has 1129 production lines, against the
800 that `[srp] file_length` in `.config/rustqual/config.toml` allows.
It carries a `qual:allow(srp, file_length=1200)` pin, so the `qual` gate fails
again once the file grows past 1200.

Split it into child modules under `provider/google/`, by responsibility and not
by size.
What the file holds today:

- the `Provider` impl;
- request construction (`create_request`);
- stream and event mapping;
- the model catalogue (`MODEL_OVERRIDES`) and model mapping;
- error mapping.

Moving the model catalogue out is likely enough on its own.
Remove the pin when the file is under 800.
A function that moves keeps its existing findings: `qual diff` follows a finding
to another file by rule and symbol.
