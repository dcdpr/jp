# Gate new untested functions on coverage data

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-11
- **Label**: domain=tooling
- **Label**: type=task

Two CI checks currently guard test coverage for new code, and both have gaps:

- The `coverage` job runs `just coverage-ci` to produce `lcov.info` and uploads
  it to Coveralls, which blocks a pull request only when total workspace
  coverage drops below a threshold.
  It does not identify which function lacks tests, and coverage from tested code
  in a large PR can mask an untested function.
- `rustqual`'s `TQ-003` (run via `qual diff`) fails when a new production
  function is not called by any test.
  Because `TQ-003` only traces direct calls in the AST, functions invoked
  through `#[serde(default = "…")]`, `clap`, or derive macros are reported as
  untested (for example, `default_true` in
  `crates/jp_config/src/conversation/label.rs`).
  The 94 existing `TQ-003` findings have not been triaged to separate genuine
  gaps from indirect callers, so `TQ-003` is excluded from the burn-down
  (T-0yh5sy9) until this ticket lands.

When passed `--coverage <lcov>`, `rustqual` evaluates two LCOV-based rules
instead of the static call graph:

- `TQ-004`: a production function with zero covered lines.
- `TQ-005`: a partially covered production function with uncovered lines.

Run through `qual diff`, both rules fail only on newly added functions and
identify the untested function by name.

## Decide

- Where to run the coverage-based rules.
  `qual diff` needs `lcov.info` for both the head and base revisions, and the
  `coverage` job is the slowest step in CI whereas `qual` finishes in seconds.
  Either `qual` downloads the head `lcov.info` artifact from `coverage` and a
  cached artifact for `base`, or the coverage rules run as a step inside the
  `coverage` job.
- Whether to enable both `TQ-004` and `TQ-005`, or only `TQ-004`.
- Whether this check replaces the Coveralls upload or runs alongside it.
- How to handle local runs, since `cargo_check` does not generate coverage data
  and findings would only surface in CI.

## Then

- [ ] Run `TQ-004` on `main` and compare its output against the 94 `TQ-003`
      findings from 2026-11-11 to verify that serde- and clap-invoked functions
      are no longer flagged.
- [ ] Wire the chosen workflow into `just qual-ci`.
- [ ] Exempt `TQ-003` in `.config/rustqual/qual.toml` unless it catches cases
      that `TQ-004` misses.
