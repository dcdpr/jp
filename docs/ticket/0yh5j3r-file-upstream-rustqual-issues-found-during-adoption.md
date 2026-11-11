# File upstream rustqual issues found during adoption

- **Status**: Todo
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
7. **`CX-003` skips functions classified as trivial.** A function with no logic
   and no calls into the project is `Trivial`, and `attach_metrics`
   (`src/adapters/analyzers/iosp/classify.rs`) drops every complexity metric of
   a trivial function, magic numbers included.
   `pub fn probe() -> u32 { 4242 }` produced no `CX-003` in CI, only `DRY-002`.
   For a function that only returns the literal this is defensible: the function
   name does the job of a `const` name.
   The same classification covers any straight-line body, though, so a literal
   passed to a std or third-party call in such a function should go unreported
   too.
   Reproduce that second case before filing, and propose limiting the exemption
   to a body that is a single literal expression.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-04T09:44:53Z

T-1dhk659 vendors rustqual into `crates/contrib/rustqual`, so these become "fix
here, offer upstream" rather than "file and wait".
Items 2 (`--compare` misses new findings), 3 (dimensions that can't be disabled)
and 4 (DRY is all-or-nothing) are covered by its follow-up, which folds `qual`'s
per-finding diff and per-rule switches into rustqual.
Item 5 (`ai-json` lacks rule ID and metric values) is a small change once it's
in-tree.
Items 1 and 6 still need reproducing.
