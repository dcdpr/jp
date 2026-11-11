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
8. **`TQ-002` doesn't follow calls through trait impls.** A test that reaches
   the code under test through `.into()` (`From`) or `to_string()` (`Display`)
   is reported as calling no production function: `from_datetime` in
   `crates/jp_cli/src/cmd/time_tests.rs` and
   `initialize_timeout_display_includes_timeout_command_and_stderr` in
   `crates/jp_mcp/src/client_tests.rs`, both marked `qual:allow(test_quality,
   no_sut)`.
   Two neighbouring tests in `client_tests.rs` assert on `to_string()` the same
   way and are not reported; find out what differs before filing.
9. **Item 1, measured.** `crates/jp_llm/src/provider/anthropic.rs` is about
   3,700 lines and counts as 22 production lines, because a `#[cfg(test)]`
   import sits on line 25.
   `openai.rs` counts as 52 and `google.rs` as 134.
   The largest files in the workspace pass the `SRP-002` length check.
   JP's own convention puts `#[cfg(test)]` on the last item of a file only, so
   the count is right for JP once T-1haqsmg enforces that; the upstream bug
   stands for workspaces without the convention.
10. **`SRP-002` doesn't name its clusters.** The SARIF message gives "N
    independent clusters" without the functions in each, although the analyzer
    has them (`cluster_names`).
    `crates/jp_cli/src/bootstrap.rs` is reported with 3 and reads as one
    responsibility; without the names the finding can't be checked or acted on.
    The cluster half is off in `.config/rustqual/config.toml` until it does.
11. **`qual:allow(complexity, magic_numbers)` above a `static` is always
    stale.** rustqual records no magic number in a `const` or `static`
    initializer, so the marker has nothing to match and is reported as
    `ORPHAN-001`.
    Correct, but the message ("no finding in window") doesn't say that the item
    kind is never checked.

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
