# Measure coverage of the review UI's browser tests

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-25
- **Label**: domain=tooling
- **Label**: relevant-rfd=139
- **Label**: relevant-rfd=140
- **Label**: type=task

`coverage-ci` measures host test runs only.
Most of `jp-review-ui` (its views, DOM bindings, selection capture,
highlighting, and seating) calls browser APIs that don't exist on the host, so
it never runs there.
The browser tests in `ui/tests/` run in headless Chrome under `test-web-ci`, and
nothing records what they cover.

Until this is done, `coverage-ci` leaves those modules out with
`--ignore-filename-regex` (`coverage_ignore` in the `justfile`), so the report
describes only what host runs can reach.
The pure modules (`route`, `layout`, `selection`, `mapping`, `composer`,
`client`, and the rest) stay in, since host tests cover them.

## What is known

`wasm-bindgen-test` can write `.profraw` files from browser tests, as an
experimental, nightly-only feature ([guide]):

- Build with `-Cinstrument-coverage -Zno-profiler-runtime
  -Clink-args=--no-gc-sections --cfg=wasm_bindgen_unstable_test_coverage` in
  `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS`.
- The profiling runtime is `minicov`, which is in `.config/supply-chain`
  already.
- `llvm-cov` can't read the coverage mapping from a `.wasm` file.
  The guide takes it from LLVM IR (`--emit=llvm-ir`) compiled with a Clang whose
  LLVM version matches Rust's.
  Where CI gets that Clang is unsolved.
- The guide's example is `cargo llvm-cov test --target wasm32-unknown-unknown`,
  with `wasm-bindgen-test-runner` as the target's runner.

None of it has been tried in this repository.

## Steps

- [ ] Produce a browser coverage report locally for `jp-review-ui`.
- [ ] Run it in CI, and merge its lcov with the host run's before it is
      uploaded.
- [ ] Remove the UI modules from `coverage_ignore`, and the variable once
      nothing is left in it.

[guide]: https://wasm-bindgen.github.io/wasm-bindgen/wasm-bindgen-test/coverage.html
