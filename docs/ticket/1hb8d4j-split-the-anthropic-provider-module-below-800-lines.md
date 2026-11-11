# Split the Anthropic provider module below 800 lines

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-11
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_llm
- **Label**: type=task

`crates/jp_llm/src/provider/anthropic.rs` has 2287 production lines, against the
800 that `[srp] file_length` in `.config/rustqual/config.toml` allows.
It carries a `qual:allow(srp, file_length=2400)` pin, so the `qual` gate fails
again once the file grows past 2400.

Split it into child modules under `provider/anthropic/`, by responsibility and
not by size.
What the file holds today:

- the `Provider` impl and credential resolution;
- request construction (`create_request`, cache control, beta features);
- stream and event mapping;
- the model catalogue (`MODEL_OVERRIDES`) and model mapping;
- error mapping.

Lower the pin with each part that moves out, and remove it when the file is
under 800.
A function that moves keeps its existing findings: `qual diff` follows a finding
to another file by rule and symbol.
