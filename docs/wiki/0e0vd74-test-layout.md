# Test Layout

- **Status**: Reference
- **Authors**: jp
- **Date**: 2026-11-26
- **Parent**: W-0e0va0y
- **Label**: domain=tooling
- **Summary**: Where a Rust module's tests live, the one declaration that
  reaches them, and how test-only code moves out of production files.

Test code lives in test files.
A production file reaches its tests through one declaration, its last item, and
no other code in it depends on `cfg(test)`.

The layout keeps test code out of what production compiles and what rustqual
counts. rustqual stops counting a file's production lines at the first line
starting with `#[cfg(test)]`, so a test-only item partway down a file hides
everything below it from the `SRP-002` file-length limit.

## One test module

A module `foo.rs` with one test module ends with:

```rust
#[cfg(test)]
#[path = "foo_tests.rs"]
mod tests;
```

`foo_tests.rs` sits beside `foo.rs` and declares no modules of its own.

A module named after a keyword drops the underscore its file name needs:
`use_.rs` has `use_tests.rs`.

## Several test modules

A module with more than one test module ends with a bare declaration, which
loads `foo/tests.rs`:

```rust
#[cfg(test)]
mod tests;
```

`foo/tests.rs` declares every other test module of `foo`, each with a `#[path]`
naming a file beside it in `foo/`:

```rust
#[path = "other_tests.rs"]
mod other_tests;

#[path = "test_support.rs"]
pub(crate) mod test_support;

// The tests themselves may follow, or live only in `other_tests.rs`.
```

Every `mod` in a test file carries a `#[path]`. rustc treats a file loaded
through a `#[path]` as owning its directory, and rustqual's resolver does not,
so an unpathed `mod` would resolve differently for the two.

A crate root, `lib.rs` or `main.rs`, owns its directory, so a bare `mod tests;`
there would load `src/tests.rs`.
It names its test directory instead: `#[path = "lib/tests.rs"]`.

Nesting a test module under `tests` changes its module path, and with it the
names of its `insta` snapshots.
Rename the snapshot files when moving one, and keep their contents.

## What counts as test-only

- A `cfg` requires `test` when it is `test`, an `all(…)` holding `test`, or an
  `any(…)` whose every branch requires `test`.
  `#[cfg(all(test, unix))]` on the closing declaration is allowed, as
  `#[cfg(test)]` is.
- `cfg(any(test, feature = "mock"))` is production code: a feature turns it on
  outside tests.
- `cfg_attr(test, …)` adds attributes rather than code, and is allowed
  anywhere.
- `cfg(not(test))` is not allowed.
  Behaviour that differs under test is a dependency to inject, not a second
  version compiled for tests.

In a production file, nothing else may require `test`: no `mod tests { … }`
block, and no test-only item, field, variant, match arm, statement, or `use`.

`#[test]` functions, including `#[tokio::test]` and its kin, appear only in
files named `tests.rs` or `*_tests.rs`.
A `test_support.rs` or a test double holds none.

`mod.rs` files are not used: `foo/mod.rs` is `foo.rs`, so `foo_tests.rs` and
`foo/tests.rs` each have one place.

The layout applies to every crate in the workspace, `crates/contrib` and
`.config/jp/tools` included.

## Moving test-only code

In order of preference:

1. Delete it, when tests can use the production API instead.
2. Inject the dependency it stands in for, when it swaps one implementation for
   another: a clock, a process runner, a provider.
3. Move it into the module's own test file, when only that module's tests use
   it.
   The test module is a child module, so it still reaches the parent's private
   items.
4. Move it into a `test_support` module declared from `foo/tests.rs`, when other
   modules' tests use it.
   They name it `foo::tests::test_support::…`, and it can still reach `foo`'s
   private items, as any descendant can.
5. Put a test double in a module of its own declared from `foo/tests.rs`, such
   as `foo/mock.rs`.

A `PartialEq` impl that only tests need, comparing `Debug` output, is replaced
at its call sites with `assert_matches!` where that loses nothing, and moved
into a test file otherwise.

## The check

`crates/internal/test-layout` checks the layout.
It walks the library, binary, and proc-macro targets of every workspace member
from their crate roots, following each `mod` declaration the way rustc resolves
it, and prints each breach as `file:line: message`:

```sh
cargo run --quiet --package test-layout
```

It exits 0 when the workspace follows the layout, 1 when it does not, and 2 when
it cannot run.
