# Upgrade the pinned nightly and fix coverage on it

- **Status**: In Progress
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

Remove the `llvm-cov` workaround comment from `rust-toolchain.toml` once
coverage runs cleanly on the new nightly.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-25T10:24:40Z

Findings from the bump to `nightly-2026-11-24` (rustc 1.102.0-nightly
20fe935d5).

**The "later versions break llvm-cov" comment is a carry-over.** It was added in
`aa630f25e` (2026-01-16) when pinning `nightly-2025-11-30` with cargo-llvm-cov
0.6.21, then copied unchanged through the bumps to `2026-02-11` (`d11f20945`,
with cargo-llvm-cov 0.8.4) and `2026-03-26` (`c7e7f98e9`).
The 03-26 pin has no known break of its own.
The original break was most likely rust-lang/rust#150151 (target-spec-json
destabilised, in nightly from 2026-01-14), which cargo-llvm-cov 0.8.4 fixed via
cargo-config2 0.1.40.

**Upstream changes since 03-26 that touch coverage:**

- rust-lang/cargo#16712: `cargo clean --target-dir` errors when the directory
  has no valid `CACHEDIR.TAG`.
  It only checks a directory that exists, and cargo-llvm-cov 0.8.6+ lets cargo
  create `target/llvm-cov-target`, so `coverage-ci`'s `cargo clean` is fine.
- Cargo 1.100 stabilised the v2 build-dir layout (rust-lang/cargo#17354).
  cargo-llvm-cov 0.9.x supports it.
  Bumped `llvm_cov_version` to 0.9.2.
- taiki-e/cargo-llvm-cov#536 (open): after switching across cargo 1.100 in the
  same target dir, stale v1 test binaries are passed to `llvm-cov` and coverage
  drops.
  CI is not affected because rust-cache keys on the rustc version.
  Locally, `rm -rf target/llvm-cov-target` once before the first `just coverage`
  on the new toolchain.
- taiki-e/cargo-llvm-cov#520 / #538 (open): the v2 layout breaks `nextest
  --archive-file`.
  We don't use archives.
- Branch coverage (`--branch`) is still nightly-only upstream
  (rust-lang/rust#79649) with no report of it being removed.
  Not yet tried here.

**Toolchain results:** the workspace builds, and `cargo test` passes all 10784
tests.
The new clippy reports roughly 300 warnings.
Most are machine-fixable: `map(..).unwrap_or_default()` → `map_or_default`
(stable since 1.98), `useless_borrows_in_formatting`, `new_instead_of_clear`,
`manual_is_variant_and`, `fetch_update` → `try_update`, unused `as _` trait
imports.
Two new pedantic test lints, `assert_is_empty` and `manual_assert_eq`, account
for a large share, and `assert_is_empty`'s suggested rewrite (`assert_eq!(v, []
as [T; 0])`) is worse than the original.
A new cargo lint, `redundant_homepage`, fired on every crate; `homepage`
(identical to `repository`) is removed.

Still to do: `just check-and-fix`, decide on `assert_is_empty`, run `just
coverage` with and without `--branch`, then drop the comment in
`rust-toolchain.toml`.

-----

- **From**: jp
- **Date**: 2026-11-25T11:34:19Z

`just coverage` failure in `dash`'s `test_only_one_driver_holds_the_queue_lock`
(`assertion failed: second.acquire_at(&path)`): not caused by the toolchain
bump.
It fails about 1 run in 4, and only in the full `dash` suite under `cargo
llvm-cov` (libtest, all tests in one process).
It never failed alone, under plain `cargo test` (10 runs), or under nextest.
CI runs coverage through nextest, so CI never hit it.

Instrumented run: right after `drop(first)`, `try_lock` returned `WouldBlock`,
and `lsof` on the lock file a few ms later showed only the probe's own
descriptor.
So the lock was held by a descriptor copy that had already gone away.
Most likely cause: a child process spawned by a test running in parallel, which
holds a copy of `first`'s descriptor until it execs and close-on-exec closes it.
A flock belongs to the open file, so the lock outlives `drop(first)` until that
copy closes.

Not a `dash` bug: a driver holds the lock for the life of the process, and other
instances retry on every poll.
The test now retries `second.acquire_at` with a 5 s deadline instead of
expecting the release on the next call.

-----

- **From**: jp
- **Date**: 2026-11-25T13:31:47Z

`--branch` builds on `nightly-2026-11-24`: the test binaries compiled with
branch instrumentation.
The run then failed in `jp_cli`, for reasons unrelated to branch coverage or the
toolchain: the command ran tests under plain `cargo test` (libtest), all of a
crate's tests in one process with the terminal on stdin.
The suite assumes nextest's one process per test with stdin closed:

- `query_model_override_persists_config_delta_through_session_targeting`:
  `build_runtime` sets the process-wide `WORKER_THREADS` once, so a second test
  in the same process panics with "double thread initialization".
- `a_failing_fork_rule_creates_no_conversation`: decides interactivity from the
  terminal, so with a TTY on stdin it prompts and waits.
- Three plugin tests fail while another test has `PATH` replaced (T-1rpjjxs#1).

`just coverage` now runs nextest the way `coverage-ci` does, and also cleans the
workspace crates' coverage artifacts first, so the local and CI runs match.

-----

- **From**: jp
- **Date**: 2026-11-25T13:45:44Z

Branch coverage is on.
Locally, `cargo llvm-cov report --branch --skip-functions` wrote 20,326 `BRDA`
records.
Without `--skip-functions`, `llvm-cov export -format=lcov` crashes in
`CoverageExporterLcov::renderRoot`: since async-trait 0.1.89 (we lock 0.1.89),
rustc emits a function record for the `#[async_trait]` async block with a branch
region and no code region, and the lcov exporter reads the first code region
unchecked (rust-lang/rust#157358, llvm/llvm-project#189169).
`--skip-functions` drops only `FN`/`FNDA`.

Both `coverage` and `coverage-ci` pass `--branch`, and the report step passes
`--skip-functions`.
The "later versions break llvm-cov" comment is removed from
`rust-toolchain.toml`.

-----

- **From**: jp
- **Date**: 2026-11-25T15:43:19Z

`async-trait` is removed.
With async-trait 0.1.89 on this toolchain, `#[async_trait]` method bodies got no
line coverage (rustc gave the generated async block the span of the `async`
keyword only), and the lcov export crashed with `--branch`.
Each trait that used the macro (`Provider`, `ProviderAuth`, `ExternalAuth`,
`BuiltinTool`, `Task`, `Executor`, `FanOutCall`, `InquiryBackend`) is now a
native-`async fn` trait plus an object-safe `Dyn*` trait implemented for every
implementor of the native one; `Handler` returns a hand-boxed `HandlerFuture`
because `typetag` needs a concrete impl per type.
Verified: the provider bodies have `DA` records again, and the report exports
without `--skip-functions` (flag and comment removed from both recipes).

`unused_async_trait_impl` (new pedantic lint) is handled per site: production
methods return `future::ready(..)` from a sync body; rmcp's `#[tool_handler]`
impls in `grizzly` and `bookworm` carry an `#[expect]`; test files with
synchronous test doubles carry a file-level `#![expect]`.
The cerebras/llamacpp/vllm "open SSE source with reconnect off" step moved into
`openai_compat::event_source`.
`DynExecutor`/`DynFanOutCall` carry `qual:allow(coupling, sit)`: their single
impl is the blanket impl.
