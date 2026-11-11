# Gate new untested functions on coverage data

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-11
- **Label**: domain=tooling
- **Label**: type=task

Two things check today that new code is tested, and neither does it well:

- The `coverage` job measures coverage (`just coverage-ci` writes `lcov.info`)
  and uploads it to Coveralls, which blocks a pull request only when total
  coverage drops below a threshold.
  It doesn't name the function that lacks a test, and a large tested change can
  carry an untested one.
- rustqual's `TQ-003` gates through `qual diff`: it fails on a production
  function the base lacks and no test calls.
  It follows direct calls only, so a function reached through `#[serde(default
  = "…")]`, clap or a derive reads as untested (`default_true` in
  `crates/jp_config/src/conversation/label.rs`).
  The 94 existing findings haven't been sorted into real and indirect ones, so
  `TQ-003` stays out of the burn-down (T-0yh5sy9) until this ticket lands.

rustqual has two rules that read coverage instead of the call graph, given
`--coverage <lcov>`:

- `TQ-004`: a production function with no covered line.
- `TQ-005`: a covered function with uncovered lines.

Run through `qual diff`, both would fail only on a function the base lacks, and
name it.

## Decide

- Whether the `qual` job should consume `lcov.info`.
  It needs the file for both the head and the base, and the `coverage` job is
  the slowest in the workflow; `qual` takes seconds today.
  Options: `qual` downloads the `coverage` job's artifact for the head and a
  cached one for the base, or the coverage rules run as a step of the `coverage`
  job.
- Whether `TQ-005` is wanted at all, or only `TQ-004`.
- Whether this replaces the Coveralls upload or sits beside it.
- `cargo_check` can't run it locally without a coverage run, so the finding
  would first show up on CI.

## Then

- [ ] Sample `TQ-004` on `main` and check it against the 94 `TQ-003` findings of
      2026-11-11: the serde and clap callers should be gone.
- [ ] Wire the chosen option into `just qual-ci`.
- [ ] Keep `TQ-003` only if it adds something `TQ-004` doesn't; otherwise
      exempt it in `.config/rustqual/qual.toml`.
