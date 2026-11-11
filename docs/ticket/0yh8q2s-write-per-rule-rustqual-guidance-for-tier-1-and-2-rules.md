# Write per-rule rustqual guidance for tier 1 and 2 rules

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-10-06
- **Label**: domain=tooling
- **Label**: type=task

Write `.config/rustqual/guidance/<RULE>.md` for each rule that gates, so a
finding tells the reader how to fix it the JP way, not just what rustqual
detected.
The gate appends the matching file to every rendered finding in CI,
`cargo_check` and the burn-down loop: the `qual` crate (T-0yh5frr) today, and
rustqual once T-1dhk659's follow-up folds `qual` into it, with the guidance
directory as a config setting.
The files themselves don't change with that move.

Rule IDs are the ones rustqual's SARIF output uses (`CX-004`, `DRY-002`,
`BP-009`, `A20`); a `/` in an ID becomes `_` in the file name, so
`iosp/violation` would be `iosp_violation.md`.

## Rules to cover

Every rule that fails the gate, since a pull request author reads the guidance
whether or not the burn-down mission (T-0yh5sy9) ever works on the rule.
The list started as the mission's tier 1 and tier 2:

- dead code and dead types (`DRY-002`, `DRY-006`);
- wildcard imports: `DRY-004` is off, clippy covers it;
- magic numbers (`CX-003`);
- boilerplate (`BP-*`): not written.
  All ten rules are exempt in `.config/rustqual/qual.toml`, and T-1dhk659 lists
  what brings each back;
- function length, cognitive and cyclomatic complexity, nesting (`CX-004`,
  `CX-001`, `CX-002`, `CX-005`);
- duplicates and fragments (`DRY-001`, `DRY-003`, `DRY-005`);
- untested functions (`TQ-003`);
- error handling (`A20`).

And the rules that gate without being the mission's: `SRP-001`, `SRP-002`,
`DEH`, `TQ-001`, `TQ-002`.
`CX-006` (`unsafe`) has no file until T-1haa4rr settles which form a site takes.

Skip rules that T-0yh5m6r exempts, and write the guidance against the thresholds
that ticket sets: `.config/rustqual/config.toml` records them, including the
150-line function limit for test code and `allow_expect = false`.

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

The files were written from rustqual's source and from the findings of
2026-11-11, and three (`A20`, `CX-003`, `DRY-002`) were rendered next to a real
finding through `cargo_check`.

Reading each file next to a sample of its rule's findings is not done here.
It happens per rule, when that rule's burn-down run starts (T-0yh5sy9, *Before a
run*), where the sample is at hand and the run's rejections test the file a
second time.
The five rules without a run are corrected as pull requests trip them.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-11T21:36:34Z

Closed on 2026-11-11 with 17 files in `.config/rustqual/guidance/`: `A20`,
`CX-001` to `CX-005`, `DRY-001`, `DRY-002`, `DRY-003`, `DRY-005`, `DRY-006`,
`SRP-001`, `SRP-002`, `DEH`, `TQ-001`, `TQ-002` and `TQ-003`.

Not written, and tracked elsewhere:

- `BP-*`: all ten rules are exempt.
  T-1dhk659 (*Exemptions to lift*) says what brings each back, and each
  returning rule needs a file.
- `CX-006` (`unsafe`): waits on T-1haa4rr, which decides the form a site takes.

Only `A20`, `CX-003` and `DRY-002` were rendered next to a real finding.
The other 14 are checked per rule when that rule's burn-down run starts
(T-0yh5sy9, *Before a run*), and the five rules without a run are corrected as
pull requests trip them.

What rustqual's messages leave out is in T-0yh5j3r, items 12 to 15.
