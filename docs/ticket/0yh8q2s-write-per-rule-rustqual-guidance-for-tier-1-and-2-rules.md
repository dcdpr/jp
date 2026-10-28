# Write per-rule rustqual guidance for tier 1 and 2 rules

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-10-06
- **Label**: domain=tooling
- **Label**: type=task

Write `.config/rustqual/guidance/<RULE>.md` for each rule that gates, so a
finding tells the reader how to fix it the JP way, not just what rustqual
detected.
The `qual` crate (T-0yh5frr) appends the matching file to every rendered finding
in CI, `cargo_check` and the burn-down loop.
Depends on T-0yh5frr for the file layout and rule IDs.

## Rules to cover

Tier 1 and tier 2, the rules the burn-down mission (T-0yh5sy9) works on:

- dead code and dead types;
- wildcard imports;
- magic numbers;
- boilerplate (`BP-*`);
- function length, cognitive and cyclomatic complexity, nesting;
- duplicates and fragments;
- untested functions;
- error handling (`unwrap`, `expect`, `panic!`).

Skip rules that T-0yh5m6r exempts, and write the guidance against the thresholds
that ticket sets.

## Each file

Keep each one short, roughly 10 to 30 lines.
Every file covers:

- **What the rule means in JP terms**, in a sentence or two.
- **The preferred fixes, in order**, tied to project conventions.
  Some examples of the kind of thing to write:
  - complexity and length: return early, extract a helper only when it has a
    name worth reading, and remember that `*_tests.rs` files sit outside
    production thresholds;
  - magic numbers: a named `const` next to its use, not a constants module;
  - error handling: propagate with `?` into the crate's typed error, and use
    `expect` only with a documented `# Panics` section.
- **What not to do.** Don't hide logic in a closure, don't split one function
  into a chain of single-use helpers, and don't touch unrelated code.
- **The escape hatch, stated once:** a `qual:allow` with a `reason:`, and only
  when the user has asked for it.

## Verify

Render a sample of real findings for each rule through `qual diff --format ai`
and check that each message gives enough to act on without looking anything else
up.
Where rustqual's own message falls short (no threshold, no actual value), add
the case to T-0yh5j3r.
