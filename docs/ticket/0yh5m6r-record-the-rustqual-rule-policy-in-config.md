# Record the rustqual rule policy in config

- **Status**: Todo
- **Kind**: Chore
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
