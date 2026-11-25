# Measure coverage of the review UI's browser tests

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-25
- **Label**: domain=tooling
- **Label**: relevant-rfd=139
- **Label**: relevant-rfd=140
- **Label**: type=task

`coverage-ci` measures host test runs only.
Most of `jp-review-ui` (views, DOM bindings, selection capture, highlighting,
and inline element seating) calls browser APIs that do not exist on the host and
therefore never executes during host tests.
The browser integration tests in `ui/tests/` run in headless Chrome under
`test-web-ci`, without coverage instrumentation.

Until browser test coverage is collected, `coverage-ci` excludes those
browser-only modules via `--ignore-filename-regex` (`coverage_ignore` in the
`justfile`) so the coverage report reflects only host-reachable code.
Pure logic modules (`route`, `layout`, `selection`, `mapping`, `composer`,
`client`, etc.) remain included and are covered by host unit tests.

## What is known

`wasm-bindgen-test` supports emitting `.profraw` files from browser tests as an
experimental, nightly-only feature ([guide]):

- Build with `-Cinstrument-coverage -Zno-profiler-runtime
  -Clink-args=--no-gc-sections --cfg=wasm_bindgen_unstable_test_coverage` in
  `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS`.
- The profiling runtime is `minicov`, which is already audited in
  `.config/supply-chain`.
- `llvm-cov` cannot read coverage mappings directly from a `.wasm` binary.
  The guide extracts them from LLVM IR (`--emit=llvm-ir`) compiled with a Clang
  binary whose LLVM version matches `rustc`'s.
  Provisioning a matching Clang in CI is still an open question.
- The guide runs `cargo llvm-cov test --target wasm32-unknown-unknown` with
  `wasm-bindgen-test-runner` configured as the target runner.

None of this has been tested in this repository yet.

## Steps

- [ ] Produce a browser coverage report locally for `jp-review-ui`.
- [ ] Run browser coverage in CI and merge its LCOV output with the host LCOV
      report before upload.
- [ ] Remove the UI modules from `coverage_ignore` (and delete `coverage_ignore`
      once empty).

[guide]: https://wasm-bindgen.github.io/wasm-bindgen/wasm-bindgen-test/coverage.html
