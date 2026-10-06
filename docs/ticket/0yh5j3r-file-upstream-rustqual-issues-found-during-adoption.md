# File upstream rustqual issues found during adoption

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-10-06
- **Label**: domain=tooling
- **Label**: type=task

rustqual problems found while adopting it, to report at
https://github.com/SaschaOnTour/rustqual/issues (pinned at v1.8.2 in the
`justfile`).
Reproduce each one on the current version before filing, and drop any that a
newer release has fixed.

1. **`SRP-002` stops counting at the first `#[cfg(test)]` anywhere in a file.**
   `count_production_lines` in `src/adapters/analyzers/srp/module.rs` breaks on
   the first line starting with `#[cfg(test)]`, so a test-only helper partway
   down a file hides all the production code below it.
2. **`--compare --fail-on-regression` misses new findings.** It treats a
   regression as `quality_score` dropping.
   Adding compliant code alongside a new finding dilutes the score enough to
   hold it level.
   A baseline that can't be parsed reports "not regressed" and exits 0 under
   `--no-fail`.
   Details are in the `qual-ci` recipe comment in the `justfile`.
3. **Dimensions that can't be disabled.** IOSP has no `enabled` key, and
   `MAGIC_NUMBER` still fires under `[complexity] enabled = false`.
   Disabling a dimension then reports its matching `qual:allow` markers as
   orphaned (`ORPHAN_SUPPRESSION`), even though the findings keep firing.
   See `.config/rustqual/dry.toml`.
4. **`DRY` is all-or-nothing.** Turning off `[duplicates]` also turns off
   dead-code (`DRY-002`) and dead-type detection.
   A per-rule switch would remove the need for a second pass.
5. **`ai-json` lacks the stable rule ID and the threshold/actual values** (for
   example `length=72 > 60`).
   Agents need both to fix a finding without a second lookup.
6. **Possible IOSP false positive.** `exponential_backoff`
   (`crates/jp_llm/src/retry.rs`) appears as a violation although it seems to
   call only std methods and do arithmetic.
   Confirm with `--verbose` that a method name matched a project function before
   filing.
