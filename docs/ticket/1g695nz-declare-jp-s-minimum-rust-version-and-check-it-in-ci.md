# Declare JP's minimum Rust version and check it in CI

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-09
- **Label**: domain=tooling
- **Label**: type=task

No crate in the workspace declares a `rust-version`, so a compiler that is too
old fails late and obscurely.
Building `jp` with Rust 1.93.0 got as far as `jp_llm`, then failed with two
E0658 errors (use of a feature 1.93 hasn't stabilized) and a compiler panic.
With `rust-version` declared, Cargo refuses an older compiler before compiling
anything, and names the version it needs.

This matters most for `cargo install --git`, the documented way to build JP from
source: it uses the user's default toolchain, not the nightly
`rust-toolchain.toml` pins.

## Which version

The minimum is somewhere from 1.94 to 1.96:

- 1.93 fails, as above.
- `rust-toolchain.toml` pins `nightly-2026-03-26`, a 1.96 nightly, and CI denies
  the `unstable-features` lint, so everything JP uses shipped in 1.96.0 at the
  latest.
- Cargo drops a nightly's pre-release suffix when comparing
  (`RustVersion::is_compatible_with`), so `rust-version = "1.96"` keeps the
  pinned nightly working.

The locked dependencies set a floor too: Cargo won't build a package whose own
`rust-version` is newer than the compiler.

## Work

- [ ] Find the minimum by checking the workspace with 1.94 and 1.95.
- [ ] Set `rust-version` in `[workspace.package]`, and opt every workspace
      member in with `rust-version.workspace = true`.
- [ ] Add a check that runs `cargo check --workspace --locked` with exactly that
      version, overriding `rust-toolchain.toml`: a `just` recipe that `rust.yml`
      runs, like its other checks.

## Notes

- No workflow or `just` recipe overrides `rust-toolchain.toml`, so nothing in CI
  builds JP with a stable compiler today.
  This check is the first.
- The workspace uses resolver 3, whose default is `incompatible-rust-versions =
  "fallback"`.
  Once a version is declared, `cargo update` prefers dependency releases that
  support it, and holds back ones that need a newer Rust until the minimum
  moves.
- When the nightly pin moves, the check shows whether the minimum has to move
  with it.
