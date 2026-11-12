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
   Running two passes means `qual:allow(dry, duplicate)` (`DRY-001`) and
   `qual:allow(dry, fragment)` (`DRY-003`) cannot be used: each marker
   suppresses its finding in the DRY pass, but the main pass (which disables
   `[duplicates]`) reports the marker as `ORPHAN-001` (item 3; verified with
   probe markers on 2026-11-12).
   The guidance files note this limitation until the second pass is removed;
   `qual:inverse` pairs are unaffected.
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
   Two neighboring tests in `client_tests.rs` assert on `to_string()` the same
   way and are not reported; find out what differs before filing.
9. **Concrete impact of item 1.** Before moving early `#[cfg(test)]` items to
   the end of each file, `crates/jp_llm/src/provider/anthropic.rs` (~3,700
   lines, `#[cfg(test)]` import on line 25) counted as 22 production lines,
   `openai.rs` as 52, and `google.rs` as 134, allowing the largest files in the
   workspace to pass the `SRP-002` length check.
   T-1haqsmg enforces placing `#[cfg(test)]` only at the end of a file in JP,
   but the upstream bug remains for workspaces without that convention.
10. **`SRP-002` does not name its clusters.** The SARIF message reports "N
    independent clusters" without listing the functions in each cluster, even
    though the analyzer computes them (`cluster_names`).
    For example, `crates/jp_cli/src/bootstrap.rs` is reported with 3 clusters
    despite having a single responsibility; without the function names, the
    finding cannot be verified or acted on.
    The cluster check is disabled in `.config/rustqual/config.toml` until the
    message includes cluster members.
11. **`qual:allow(complexity, magic_numbers)` on a `static` or `const` is
    reported as `ORPHAN-001` without explaining why.** `rustqual` does not flag
    magic numbers inside `const` or `static` initializers, so a suppression
    marker on those items has no finding to match.
    Reporting `ORPHAN-001` is accurate, but the message ("no finding in window")
    does not indicate that `const` and `static` items are already exempt from
    `CX-003`.
12. **SARIF messages omit fields the analyzer already computes.** The guidance
    files (T-0yh8q2s) hardcode the missing context:
    - `CX-001`, `CX-002`, `CX-004`, and `CX-005` report the measured value
      without the threshold (`ComplexityFinding` stores both).
    - `A20` reports only a count ("error-handling smells 1") without naming
      whether `unwrap`, `expect`, `panic!`, `unreachable!`, or `todo!` triggered
      it.
    - `DRY-001`, `DRY-003`, and `DRY-005` report "N other function(s)" without
      listing them, even though each finding stores its `participants` (only the
      text report prints them).
    - `DRY-002` omits the `uncalled` / `testonly` classification and the
      remediation suggestion.
13. **`DRY-002` and `DRY-006` count calls, not references.** A function passed
    as a value is flagged as dead code: `http_handler` in
    `crates/jp_attachment_http_content/src/lib.rs` (used to initialize a
    `static`) and `parse_spec` in `crates/jp_cli/src/cmd/compact_flag.rs` (a
    clap value parser).
    A `static` registered via `#[distributed_slice(HANDLERS)]` (`HANDLER` in the
    attachment crates), which is read by the linker, is also flagged.
    Neither rule accepts `qual:allow`, and `qual:api` is rejected on private
    items.
14. **`DRY-005` groups `match` expressions by shape, ignoring the matched
    type.** `group_repeated_patterns`
    (`src/adapters/analyzers/dry/match_patterns.rs`) groups by a hash of the
    normalized `match` alone.
    Normalization strips path prefixes from patterns and erases literal values,
    so every `match` that maps the same number of unit variants to string
    literals lands in the same group regardless of the enum.
    For example, "repeated match on Self (6 occurrences)" groups
    `PathRoot::as_str` in `bookworm` (whose arm patterns start with
    `PathRoot::`) together with `<Status as Display>::fmt` in `ticket`: the type
    name in the message is extracted from the second-to-last path segment of an
    arm in one `match` of the group (`extract_enum_name`), not used in the
    grouping key.
    Propose including the matched type in the grouping key (resolving `Self` to
    the enclosing `impl` type) and skipping `match` expressions whose matched
    type cannot be inferred from the arm patterns.
    The detector also ignores `[duplicates] enabled = false`: `run_dry_pass`
    (`src/app/secondary.rs`) gates it on `detect_repeated_matches` alone, so
    JP's main pass runs it too.
15. **`TQ-001` passes any test that contains a function call.**
    `detect_assertion_free_tests` (`src/adapters/analyzers/tq/assertions.rs`)
    treats any function or method call in the test body as an implicit
    assertion.
    As a result, `#[test] fn it_works() { run_thing(); }` (the rustqual
    documentation's own example of what `TQ-001` catches) passes, and only tests
    with neither assertion macros nor calls are reported.

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
