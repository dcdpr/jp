# Record the rustqual rule policy in config

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-10-06
- **Label**: domain=tooling
- **Label**: type=task

Decide, for each rustqual rule, whether JP enforces it, tunes it, or exempts it.
Record each decision and its reason as a comment in `.config/rustqual/`, in the
style the existing configs already use.
The decisions belong to the maintainers.

**For an assistant picking this up:** apply the items under *Decided* as
written.
For each item under *To decide*, don't choose.
Draft the entry instead: the current finding count, three or four real examples
from the workspace, and the options with what each one costs.
Then stop and ask the user to decide before writing it into the config.
The PR is where the decisions get reviewed.

Depends on T-0yh5frr for the `exempt` policy list.

This has to land before the burn-down mission runs, so the loop doesn't refactor
code to satisfy rules JP never adopted.

## Decided

- **IOSP: exempt (option A).** rustqual can't disable it, so it goes in the
  `qual` crate's `exempt` list rather than behind suppressions (862 violations,
  6.2% of functions, which is over the 5% `max_suppression_ratio`).
  Revisit if, after the tier 2 burn-down (complexity, length, nesting), reviews
  still turn up functions within those limits that mix orchestration and logic
  badly.
  The two alternatives:
  - **B:** gate IOSP on new code only;
  - **C:** burn it down fully, with `allow_recursion = true`.
- **`[srp] file_length = 800`.** rustqual counts only code lines: blank lines,
  `//`, `///` and block comments don't count, and counting stops at the first
  `#[cfg(test)]`.
  The default of 300 is rustqual's own house style.
  Note that `srp_module_warnings` also covers the independent-cluster cohesion
  check, so it won't drop to zero from the length change alone.

## To decide

- **`[tests]` thresholds.** `*_tests.rs` files have no `#[cfg(test)]`, so every
  code line counts and they inherit 800 and the 60-line function limit.
  JP's tests are deliberately self-contained, which makes them long.
- **`allow_expect`.** `EventId::random` uses `.expect()` with a documented `#
  Panics` section, a deliberate pattern.
  Decide whether `.expect()` is allowed while `.unwrap()` keeps firing (194
  error-handling findings in total).
- **`unsafe`** (74 findings).
  Suppress audited FFI and syscall sites with `qual:allow(unsafe)`, which
  doesn't count toward the suppression ratio, or exempt the rule.
- **`[boilerplate] accepted_display_idioms`.** Declare JP's house style for
  trivial `Display` impls, so `BP-002` enforces that style instead of firing on
  it.
- **Any rule** whose findings the maintainers consider noise after looking at
  samples.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-04T09:44:52Z

Applied in the T-0yh5frr PR, in `.config/rustqual/config.toml` and `qual.toml`:

- IOSP exempt (`qual.toml`).
- `[srp] file_length = 800`.
- `[tests]`: no file-length limit for test code (`file_length = 1_000_000`,
  since rustqual can't turn the check off for tests alone), and
  `max_function_lines = 150`.
  On 2026-10-07, 185 test functions were over 60 lines, 14 over 150. rustqual
  does resolve `#[cfg(test)] #[path = "…_tests.rs"]`, so these thresholds reach
  the `*_tests.rs` files.
- `allow_expect = false`.
  A deliberate `.expect()` takes a `qual:allow` with a reason; raise
  `max_suppression_ratio` if those markers ever approach it.
- `unsafe`: option A. Every site gets an audited `qual:allow(unsafe) reason:
  …`, then `CX-006` joins `enforced`.
  Recorded as a comment in `qual.toml`, not yet in the list: with 74 unmarked
  sites (41 production, 33 test) an enforced `CX-006` would fail every run.
  The marking is a ticket of its own.

Still open:

- **`[boilerplate] accepted_display_idioms`.** The 56 `BP-002` findings use
  three idioms: `write_macro` 32, `write_str` 12, `delegation` 12.
  They aren't interchangeable (delegation honours padding flags, `write!`
  formats several values), so a single house idiom would make code worse.
  Waiting on T-1dhk659: a vendored rustqual can narrow `BP-002` to the
  boilerplate case.
- **Noise review.** Not noise: `CX-004`, `CX-003`, `CX-001`/`CX-002`, `CX-005`,
  `A20`.
  Samples were reviewed for the rest; candidates:
  - `SLM` overlaps clippy's `unused_self` and ignores
    `#[expect(clippy::unused_self)]`; `NMS` gave false positives on
    `as_text_mut` and a `mem::take` mutation.
    Exempt both and rely on clippy?
  - `TQ-002`: all three samples call the code under test through `Into`, serde
    or `Display`.
  - `TQ-003`: run without coverage, so it can't follow serde or clap callers
    (`default_true`, `read_ids`).
  - `IET`: 3 of 7 are serde `serialize`/`deserialize` pairs.
  - `SRP-001`: 37 of 46 are LCOM4=2, some from a constructor that never touches
    `self` (`ApprovalStore::load`).
  - `SRP-002`: 39 production findings, split between length (over 800) and
    independent clusters.

The `qual-report.json` behind these numbers is not committed; regenerate with
`qual report --format json`.
