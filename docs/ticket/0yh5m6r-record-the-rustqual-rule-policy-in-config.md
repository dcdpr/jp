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

-----

- **From**: jp
- **Date**: 2026-11-11T12:54:27Z

Decided on 2026-11-11, applied in `.config/rustqual/`:

- **Clippy first.** When a clippy lint does what a rustqual rule does, as well
  or better, the rustqual rule is off.
  That includes lints not yet enabled, nightly-only ones too.
  Off under this rule: `SLM` (`unused_self`), `NMS`
  (`needless_pass_by_ref_mut`), `DRY-004` (`wildcard_imports`; all 10 findings
  were enum globs, which the workspace allows on purpose), `BP-005`
  (`derivable_impls`), `SRP-003` (`too_many_arguments`, which `.clippy.toml`
  sets to 10 where rustqual fired above 5).
  The rules that need a lint turned on first are in T-1haa4rr.
- **`TQ-002`: kept.** Of the three findings, one was right:
  `from_env_source_is_env_with_key` in `crates/jp_cli/src/session_tests.rs`
  built a value and compared it with an identical literal, so it is deleted.
  The other two reach the code under test through `From` and `Display` and carry
  `qual:allow(test_quality, no_sut)` (T-0yh5j3r item 8).
- **`TQ-003`: exempt.** T-1ha99kp replaces it with a gate on coverage data.
- **`IET` and `BP-002`: exempt** until the vendored rustqual fixes them;
  T-1dhk659 lists both under *Exemptions to lift*.

Still open: `SRP-001` and `SRP-002`.

- `SRP-001` (50): 39 are LCOM4 = 2.
  The score is `0.4 × lcom4 + 0.25 × fields/12
  - 0.15 × methods/20 + 0.2 × fan_out/10`, firing at 0.6, and with
    `lcom4_threshold = 2`any LCOM4 of 2 or more scores the full 0.4. A one-field
    clap command whose`run`doesn't read`self` and calls ten functions fires
    (`Show`, `Unarchive`).
- `SRP-002` (46): 24 over 800 lines, 22 on clusters alone.
  The length count is wrong for files with an early `#[cfg(test)]` (T-0yh5j3r
  item 9).

-----

- **From**: jp
- **Date**: 2026-11-11T13:47:04Z

Decided later on 2026-11-11:

- **`TQ-003`: gating again.** Coveralls only blocks when total coverage drops
  below a threshold, so `TQ-003` is the one check that names a new untested
  function.
  It is out of `exempt`, and stays out of the burn-down until T-1ha99kp moves
  the check onto coverage data.
- **`SRP-001`: `lcom4_threshold = 3`.**
- **`SRP-002`: length half kept, cluster half off** (`max_independent_clusters`
  out of reach) until a finding names its clusters.
  The length count stops at the first `#[cfg(test)]`, which is right under JP's
  convention once T-1haqsmg enforces it.
- **Stale markers removed.** The three `qual:allow(complexity, magic_numbers)`
  markers on the provider model catalogues sat above `static` items, where
  rustqual records no magic number.
  Removing them added no finding.

Nothing under *To decide* is open now.
The clippy side continues in T-1haqqbv and T-1haa4rr.
