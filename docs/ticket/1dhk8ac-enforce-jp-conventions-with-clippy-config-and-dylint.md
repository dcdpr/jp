# Enforce JP conventions with clippy config and dylint

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-04
- **Label**: domain=tooling
- **Label**: type=pre-rfd

Several JP conventions are written down (contributor docs, assistant
instructions) but nothing checks them, so they are caught case by case in
review.
Some can be enforced with configuration clippy already reads; the rest need
project-specific lints, which `dylint` makes possible.

Neither overlaps with rustqual. rustqual measures structure (size, complexity,
duplication) from syntax alone; these are rules about what code may do and
where, and most of them need types.

## Step 1: clippy's `disallowed-*` lists

`.clippy.toml` has `disallowed-methods`, `disallowed-types` and
`disallowed-macros`, all empty.
A ban there applies to the whole workspace; the module that owns the banned
thing opts back in with an inner `#![expect(clippy::disallowed_methods, reason =
"…")]`.
Candidates:

- `std::io::stdout` and `std::io::stderr` outside `jp_printer` and binaries'
  `main`.
  `clippy::print_stdout` and `print_stderr` only see the macros.
  `jp_cli` writes to `io::stderr()` directly in `bootstrap.rs`, `cmd/target.rs`
  (4 sites) and `cmd/workspace/target.rs` (2), the bug class in T-0ffsv2r.
- The run-to-completion `std::process::Command` methods (`output`, `status`)
  outside `jp_process`, which is the `ProcessRunner` seam.
  `spawn` stays allowed: long-lived processes (MCP servers, ACP, plugins) are
  outside `jp_process`'s scope.
  Direct callers today include `jp_cli/src/lib.rs`, the ticket plugin and the
  gui plugin.
- `std::fs` in `jp_config` outside its `fs` and `util` modules.

Check `clippy::absolute_paths` (a restriction lint) for the "import, don't
qualify" rule before writing anything for it.

## Step 2: a dylint library

For rules clippy can't express.
`exodus_lints`, in another JP-managed workspace, is a working example: a
`cdylib` with one late lint pass and 17 rules.
Each diagnostic carries a note on why and a `help` naming the section of the
conventions document it enforces, and every rule has `dylint_testing` UI tests.
Candidates for JP:

- A `use` inside a function body, outside `#[cfg(...)]` code.
- An inline `#[cfg(test)] mod tests { … }` instead of a `#[path =
  "…_tests.rs"]` file.
- A conversion before `?` that `?` already does: `.ok_or_else(|| E::from(x))?`,
  `.map_err(Into::into)?`.
- Prose in a tracing field (exodus's `PROSE_IN_TRACE_FIELD`), once D15 and D32
  settle what a field may carry.

## Costs

- The lint crate builds on the workspace's own nightly, never a second one.
  `exodus_lints` pins `nightly-2026-10-01` and JP `nightly-2026-03-26`, so this
  depends on T-1dhs4hp moving JP forward.
  It adds `rustc-dev` to the toolchain's components.
- `clippy_utils` is released for one exact nightly, and rustc's internal API
  changes between nightlies.
  Every toolchain bump has to land on a nightly that has a `clippy_utils`
  release and port the lint crate to any rustc API changes.
- Requires a separate Cargo workspace (it cannot be a member of the root
  workspace) and a per-target `dylint-link` linker configuration.
  `exodus_lints` configures only `aarch64-apple-darwin`; CI runs Linux and
  Windows.
- Requires `cargo-dylint` and `dylint-link` in CI, plus an extra type-checking
  pass over the workspace (roughly the cost of the `lint` job).

Step 1 has none of these costs, so it goes first; it may cover enough that step
2 can wait.
Whether to adopt step 2, and with which rules, needs an RFD.
