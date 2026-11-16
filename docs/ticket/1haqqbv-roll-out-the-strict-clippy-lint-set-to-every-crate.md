# Roll out the strict clippy lint set to every crate

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-11
- **Label**: domain=tooling
- **Label**: type=pre-rfd

Roll out the strict Clippy lint set currently enabled on five crates to the
entire workspace incrementally: existing code continues to pass, new code is
checked against the full lint set immediately, and existing violations are
burned down over time.

## Current State

- `[workspace.lints.clippy]` enables `all` and `pedantic` with twelve allowed
  lints.
  Every workspace crate inherits this configuration except
  `crates/contrib/xct2cli`, which maintains its own lint list to stay close to
  upstream.
- Five crates (`jp_config`, `jp_conversation`, `jp_md`, `jp_printer`, and
  `jp_pty`) add a `#![warn(…)]` block in `lib.rs` enabling `clippy::nursery`,
  `missing_docs`, `rustdoc::all`, and restriction lints such as `unwrap_used`,
  `allow_attributes`, `missing_docs_in_private_items`, `todo`, `unimplemented`,
  and `use_debug`.
- `just lint-ci` runs Clippy with `--deny warnings`, so enabling a lint
  workspace-wide requires fixing every existing violation in the same commit.

## Problem: No Incremental Rollout

`rustqual` had the same adoption barrier, which `qual` solved by diffing
findings against the base revision (T-0yh5npc).
Three approaches can provide incremental rollout for Clippy:

1. **Diff gate.** Run non-clean lints in a second Clippy pass without `--deny
   warnings`, and compare its diagnostics against the base revision by `(lint,
   file, symbol)` as `qual diff` does for `rustqual`.
   New code is checked against every lint immediately without adding suppression
   attributes to the codebase, and CI, `cargo_check`, guidance files, and the
   burn-down mission (T-0yh5sy9) work the same way as they do for `rustqual`.
   Once a lint reaches zero violations, it moves into `[workspace.lints]`, which
   serves the same role as the `enforced` list in `qual.toml`.
   Cost: requires a second Clippy pass over both `base` and `head`, plus a
   Clippy JSON diagnostic parser in `qual` (which T-1dhk659 otherwise plans to
   fold into `rustqual` and delete).
2. **Per-site `#[expect]` attributes.** Enable the lints in `[workspace.lints]`
   and use Clippy's JSON output to insert `#[expect(clippy::..., reason =
   "...")]` on every existing violation.
   New code is checked immediately, and `rustc` warns when an `#[expect]`
   attribute becomes stale after a fix.
   Cost: adds thousands of attributes for high-volume lints, and cannot suppress
   module-wide lints such as `missing_docs_in_private_items` at individual
   sites.
3. **Crate-level `#![allow]` lists.** Move the strict lint set into
   `[workspace.lints]` and add a crate-level `#![allow(…)]` in `lib.rs` for
   each crate that still has violations.
   Cost: requires no extra tooling, but new code in crates that still allow a
   lint is not checked, allowing violations to accumulate during the burn-down.

## To settle in the RFD

- Which rollout mechanism (or combination) to use.
- The target lint set: the existing five-crate block, all of
  `clippy::restriction` minus an allowlist, or a curated subset.
- Whether `crates/contrib/*` crates that track an upstream repository are in
  scope.
- How to structure the burn-down mission (for example, one run per lint as
  T-0yh5sy9 does per `rustqual` rule, and whether both share a mission script).
- How to sequence this rollout with the `rustqual` burn-down, since T-1haa4rr
  migrates five `rustqual` rules to Clippy lints and running both would touch
  the same call sites twice.
