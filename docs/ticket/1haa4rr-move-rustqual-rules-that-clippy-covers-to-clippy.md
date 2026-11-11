# Move rustqual rules that clippy covers to clippy

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-11
- **Label**: domain=tooling
- **Label**: type=task

Per T-0yh5m6r, whenever a Clippy lint covers the same check as a `rustqual`
rule, JP disables the `rustqual` rule and relies on Clippy (including lints not
yet enabled in the workspace).
Clippy has full type information and honors `#[expect]`, whereas `rustqual`
operates on syntax alone.

Five `rustqual` rules are already disabled because the corresponding Clippy lint
is active (`SLM`, `DRY-004`, `BP-005`, `SRP-003`) or because `rustqual` produced
only false positives (`NMS`).

The remaining rules require enabling a Clippy lint first.
Because `just lint-ci` runs Clippy without a baseline diff, enabling a lint
workspace-wide requires either fixing or annotating (`#[expect(..., reason =
"...")]`) every existing violation, or gating the lint through a diff tool
(T-1haqqbv).
Counts below are from `qual report` on 2026-11-11.

| rustqual rule            | clippy lint                                                                                 | Existing sites                  | Notes                                                                                                                                      |
| ------------------------ | ------------------------------------------------------------------------------------------- | ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| `NMS` (off)              | `needless_pass_by_ref_mut` (nursery)                                                        | 20 from clippy, all in `jp_cli` | Mostly `run(self, ctx: &mut Ctx)` on commands that only read `ctx`. Skips exported functions unless `avoid-breaking-exported-api = false`. |
| `A20` error handling     | `unwrap_used`, `expect_used`, `panic`, `unreachable`, `todo`, `unimplemented` (restriction) | 203                             | `.clippy.toml` already sets `allow-unwrap-in-tests`. `jp_config` and `jp_conversation` enable `unwrap_used` at the crate level.            |
| `CX-006` `unsafe`        | `undocumented_unsafe_blocks` (restriction)                                                  | 74                              | Requires a `// SAFETY:` comment instead of the `qual:allow(unsafe) reason:` marker planned in T-0yh5m6r, replacing the `enforced` plan.    |
| `CX-005` nesting depth   | `excessive_nesting` (enabled once `excessive-nesting-threshold` is set)                     | 31                              | Counts all nested blocks, so finding counts differ slightly from `CX-005`.                                                                 |
| `CX-004` function length | `too_many_lines` (pedantic, currently at its default of 100)                                | 312 over 60                     | Uses a single threshold for all code, whereas T-0yh5m6r sets 60 for production code and 150 for tests. Both checks currently run.          |

Rules without a Clippy equivalent stay in `rustqual`: `CX-001` and `CX-002`
(`clippy::cognitive_complexity` does not measure cyclomatic or cognitive
complexity accurately), `CX-003`, `DRY-001`/`002`/`003`/`005`/`006` (`rustc`'s
`dead_code` lint stops at crate boundaries), `SRP-001`/`002`, `TQ-001`/`002`,
`DEH`, and the remaining `BP-*` rules.

## Decide per row

Decide whether to migrate the rule to Clippy and how to sequence it with the
burn-down (T-0yh5sy9): burn down existing violations under `rustqual` before
switching, or switch first and annotate remaining sites.

## Then, per row moved

- Enable the lint in `[workspace.lints.clippy]` and configure its threshold in
  `.clippy.toml`.
- Fix or annotate existing violations in the same change.
- Disable the `rustqual` rule in `config.toml` (if it has a toggle) or add it to
  `exempt` in `qual.toml`.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-11T13:47:04Z

These migrations depend on T-1haqqbv, which decides how to roll out new Clippy
lints without fixing every existing violation in a single commit.
If T-1haqqbv adopts a diff gate, each row can be added to the gated lint set,
burned down, and then promoted to `[workspace.lints]`.
