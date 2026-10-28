# Add a qual crate that diffs rustqual findings by identity

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-10-06
- **Label**: domain=tooling
- **Label**: type=task

A new internal crate, `crates/internal/qual`, that turns rustqual reports into
individual findings, compares two reports, and renders the result.
CI, `cargo_check` and the burn-down loop harness all call it, so the gate and
its messages behave the same in every place it runs.

## Why

The gate today compares per-category counts in `.config/rustqual/baseline.json`
and `baseline-dry.json`.
Those files record a location only for IOSP violations (`violation_details`).
Every other finding (about 2,000 of the ~3,200 distinct ones) is stored as a
count.
That is why CI can say `srp_module_warnings: 207 -> 208` but not which file.
A committed baseline also goes stale as soon as `main` moves (see T-0yb2pgw).

## Scope

- **Parse** both passes (`config.toml` and `dry.toml`) into findings keyed by
  `(rule, file, symbol)`.
  Line numbers are not part of the key, so editing code above a function causes
  no churn.
  Duplicate keys count as a multiset.
  IOSP appears in both passes, so de-duplicate it.
  This ticket picks the input format (`json`, `sarif` or `ai-json`): whichever
  gives a stable rule ID and symbol most cleanly.
- **Diff** a base report against a head report and return the new findings.
  Before comparing, map renamed files using `git diff -M` between the two
  revisions.
- **Base analysis:** run rustqual on `git worktree add --detach <base>`.
  Cache the report, keyed by base sha, rustqual version and config hash.
  A run takes a few seconds, so the cache is a convenience, not a requirement.
- **Policy**, read from `.config/rustqual/` (exact file decided here):
  - `exempt`: rules that never gate.
    IOSP goes here, because rustqual has no way to turn it off.
  - `enforced`: rules that have reached zero on `main`.
    Any finding of an enforced rule fails, whether or not it appears in the
    diff.
    Rules are only ever added to this list; it never shrinks.
- **Render** the same findings as plain text, GitHub annotations (`::error
  file=…,line=…::`), a Markdown table for `$GITHUB_STEP_SUMMARY`, and a
  compact AI-oriented format.
  Each finding carries rustqual's message plus the guidance from
  `.config/rustqual/guidance/<RULE>.md` when that file exists.
- **CLI:**
  - `qual diff --base <rev> [--format …]` exits non-zero on new or enforced
    findings.
  - `qual report` lists every current finding (the burn-down picker needs this).
- **Library API:** jp-tools depends on the crate the same way it depends on
  `ticket`.

## Fail closed

A report that can't be parsed, or a rustqual run that fails, is an error, never
a pass.
This replaces the canary in `qual-ci`: unit tests prove that a new finding fails
the gate and that a malformed report is an error.

## Tests

Keep the core pure: parsing, rename mapping, diff, policy and rendering are
tested against fixed fixture reports.
Only the thin shell around them spawns rustqual and git.

## Out of scope

Wiring into CI, `cargo_check` or the harness.
Those are separate tickets that depend on this one.
