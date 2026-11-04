# Upgrade the pinned nightly and fix coverage on it

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-11-04
- **Label**: domain=tooling
- **Label**: type=task

`rust-toolchain.toml` pins `nightly-2026-03-26`, with the comment "later
versions break llvm-cov, need to investigate".
The pin was set in `c7e7f98e9` and the investigation never happened, so the
workspace is over seven months behind.
Move to a current nightly and make coverage work on it.

T-1dhk8ac depends on this: dylint lints have to build on the workspace's own
toolchain, not a second nightly.

## Coverage today

- `cargo-llvm-cov` is pinned at 0.8.7 (`llvm_cov_version` in the `justfile`).
- `just coverage` and `coverage-ci` run without `--branch`; the `justfile`
  carries `FIXME: Branch coverage seems to have broken recently?`.
- CI uploads `lcov.info` to Coveralls.

What exactly broke on later nightlies isn't recorded.
Start by reproducing it.

## Steps

1. Bump the nightly and run `just coverage` with the current `cargo-llvm-cov`.
   It may already be fixed upstream in either.
2. If it still fails, record the failure here, check `cargo-llvm-cov`'s and
   rustc's issue trackers, and fix or work around it.
3. Try `--branch` again while there.
4. Fix whatever the new nightly's rustc and clippy report, and update anything
   else tied to the toolchain (`rust-analyzer`, `llvm-tools`, CI caches).

The toolchain comment in `rust-toolchain.toml` goes once the reason for it does.
