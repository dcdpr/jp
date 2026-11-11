# Move rustqual rules that clippy covers to clippy

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-11
- **Label**: domain=tooling
- **Label**: type=task

The rule, from T-0yh5m6r: when a clippy lint does what a rustqual rule does, as
well or better, the rustqual rule is off and clippy does the job.
That includes lints the workspace hasn't enabled yet, nightly-only ones too.
clippy sees types and honours `#[expect]`; rustqual reads syntax.

Off already, because the clippy lint was on (`SLM`, `DRY-004`, `BP-005`,
`SRP-003`), or because the rustqual rule was wrong (`NMS`).

What is left needs a lint turned on. clippy has no base revision to compare
against: a lint that is on fails `lint-ci` for every existing site, so each one
below lands with its sites fixed or marked `#[expect(…, reason = "…")]`.
Counts are from `qual report` on 2026-11-11.

| rustqual rule            | clippy lint                                                                                 | Existing sites                  | Notes                                                                                                                                             |
| ------------------------ | ------------------------------------------------------------------------------------------- | ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| `NMS` (off)              | `needless_pass_by_ref_mut` (nursery)                                                        | 20 from clippy, all in `jp_cli` | Mostly `run(self, ctx: &mut Ctx)` on commands that only read `ctx`. Skips exported functions unless `avoid-breaking-exported-api = false`.        |
| `A20` error handling     | `unwrap_used`, `expect_used`, `panic`, `unreachable`, `todo`, `unimplemented` (restriction) | 203                             | `.clippy.toml` already sets `allow-unwrap-in-tests`. `jp_config` and `jp_conversation` name `unwrap_used` in a crate-level attribute.             |
| `CX-006` `unsafe`        | `undocumented_unsafe_blocks` (restriction)                                                  | 74                              | Asks for a `// SAFETY:` comment, where T-0yh5m6r planned `qual:allow(unsafe) reason:` on every site. Replaces the `enforced` plan in `qual.toml`. |
| `CX-005` nesting depth   | `excessive_nesting`, on once `excessive-nesting-threshold` is set                           | 31                              | Counts nested blocks, so the two numbers won't match exactly.                                                                                     |
| `CX-004` function length | `too_many_lines` (pedantic, on at its default of 100)                                       | 312 over 60                     | One threshold for all code, where T-0yh5m6r chose 60 for production and 150 for tests. Both run today.                                            |

Not covered by clippy, so they stay with rustqual: `CX-001`/`CX-002` (clippy's
`cognitive_complexity` says of itself that it doesn't measure it), `CX-003`,
`DRY-001`/`002`/`003`/`005`/`006` (rustc's `dead_code` stops at the crate
boundary), `SRP-001`/`002`, `TQ-001`/`002`, `DEH`, the other `BP-*`.

## Decide per row

Whether to move it, and in which order relative to the burn-down (T-0yh5sy9):
fixing the sites first under rustqual and switching once they are gone, or
switching now and marking what remains.

## Then, per row moved

- Enable the lint in `[workspace.lints.clippy]`, with its thresholds in
  `.clippy.toml`.
- Fix or mark the existing sites in the same change.
- Disable the rustqual rule, in `config.toml` where it has a switch and in
  `qual.toml`'s `exempt` where it has none.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-11T13:47:04Z

The rows here wait on T-1haqqbv, which settles how a clippy lint is turned on
without fixing every site in the same change.
If that lands as a diff gate, each row becomes "add the lint to the gated set,
burn it down, then move it to `[workspace.lints]`".
