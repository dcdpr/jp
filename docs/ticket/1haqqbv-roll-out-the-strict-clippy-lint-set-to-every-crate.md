# Roll out the strict clippy lint set to every crate

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-11
- **Label**: domain=tooling
- **Label**: type=pre-rfd

Every crate should build under the strict lint set that five crates carry today,
and reach it gradually: existing code keeps passing, new code is held to the
lints at once, and the debt is paid off by an unattended mission.

## Today

- `[workspace.lints.clippy]` turns on `all` and `pedantic` with twelve lints
  allowed, and every crate inherits it except `crates/contrib/xct2cli`, which
  keeps its own shorter list to stay close to upstream.
- `jp_config`, `jp_conversation`, `jp_md`, `jp_printer` and `jp_pty` add a
  `#![warn(…)]` block in `lib.rs`: `clippy::nursery`, `missing_docs`,
  `rustdoc::all`, and restriction lints such as `unwrap_used`,
  `allow_attributes`, `missing_docs_in_private_items`, `todo`, `unimplemented`
  and `use_debug`.
- `just lint-ci` runs clippy with `--deny warnings`, so a lint is either clean
  everywhere it applies or it fails the job.
  That is what has kept the block out of the other crates: turning a lint on
  means fixing every site in the same change.

## The gap: no way to pay off gradually

rustqual had the same problem and `qual` solved it by comparing findings against
the base commit (T-0yh5npc).
Three ways to get the same for clippy:

1. **Diff gate.** Run the lints not yet clean in a second clippy pass that
   doesn't deny, and compare its diagnostics against the base by `(lint, file,
   symbol)`, as `qual diff` does for rustqual.
   New code is held to every lint from day one, no marker goes into the code,
   and CI, `cargo_check`, the guidance files and the burn-down mission
   (T-0yh5sy9) work for clippy as they do for rustqual.
   A lint that reaches zero moves into `[workspace.lints]`, which is the
   `enforced` list of `qual.toml` in another place.
   Costs: a second clippy pass over the base and the head, and a reader for
   clippy's JSON diagnostics in `qual`, which T-1dhk659 plans to fold into
   rustqual and delete.
2. **A marker per site.** Turn the lint on and put `#[expect(clippy::x, reason =
   "…")]` on every existing site, inserted from clippy's JSON output.
   New code is checked at once, and `#[expect]` reports itself when its site is
   fixed.
   Costs: thousands of attributes for the high-count lints, and it doesn't work
   at all for `missing_docs_in_private_items`.
3. **Crate by crate.** Move the block to `[workspace.lints]` and give each crate
   that isn't clean a `#![allow(…)]` naming the lints it still fails.
   Cheapest to start.
   New code in a crate that still allows a lint isn't checked for it, so the
   debt can grow while it is being paid.

## To settle in the RFD

- Which of the three, or which mix.
- The target lint set: the block as it is, all of `restriction` minus a deny
  list, or something between.
- Whether `contrib` crates that track an upstream are in scope.
- The mission: one run per lint, as T-0yh5sy9 does per rustqual rule, and
  whether the two share one mission script.
- Order against the rustqual burn-down, since T-1haa4rr moves five rustqual
  rules onto clippy lints and both would otherwise fix the same sites twice.
