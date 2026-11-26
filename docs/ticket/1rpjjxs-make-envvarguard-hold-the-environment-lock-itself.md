# Make EnvVarGuard hold the environment lock itself

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-25
- **Label**: domain=tooling
- **Label**: package=jp_test
- **Label**: type=task

`jp_test::env_var::EnvVarGuard` restores a variable on drop, but it doesn't
serialize anything.
Every test that holds one must also be marked `#[serial(env_vars)]`, and the
`unsafe` blocks inside the guard are sound only if every caller follows that
convention.
Nothing checks that they do: a test that forgets the attribute writes the
environment while other tests read it.

Have the guard take a process-wide mutex itself, so holding a guard means
holding the lock:

```rust
let _env = EnvVarGuard::set_all(&[("GIT_CONFIG_GLOBAL", ""), ("GIT_CONFIG_SYSTEM", "")]);
```

A test then takes one guard for every variable it changes.
Two separate guards would deadlock, because a std mutex can't be taken twice by
the same thread, so `set_all` (or a builder) replaces holding several guards in
one test.

## Decide

- How tests that only *read* the environment are serialized.
  They hold no guard, so they still need `#[serial(env_vars)]`, or a shared read
  lock from the same mutex (an `RwLock`), or nothing if what they read is never
  written.
- Whether the guard's lock replaces `#[serial(env_vars)]` everywhere, or both
  stay.
  The two are separate locks, so mixing them doesn't serialize anything: a
  guard-holding test without the attribute runs alongside a
  `#[serial(env_vars)]` test that calls `set_var` directly.
- How to handle a test that panics while holding the lock.
  Recover from the poisoned mutex (`unwrap_or_else(PoisonError::into_inner)`)
  rather than fail every later test.

## Then

- [ ] Add the lock and `set_all` to `EnvVarGuard`.
- [ ] Convert every guard user (`jp_cli`, `jp_config`, `jp_llm`, `dash`,
      `overwrite`, `jp-serve-chat`, `jp-ticket`) to one guard per test.
- [ ] Convert the tests that call `env::set_var`/`remove_var` directly to the
      guard: `crates/jp_workspace/src/lib_tests.rs` and
      `crates/jp_ffi/src/lib_tests.rs`, each with its own hand-written
      restore-on-drop helper.
- [ ] Update the guard's `SAFETY` comments to rely on the lock, not on the
      attribute.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-25T13:31:46Z

A concrete instance, seen running `jp_cli`'s tests under plain `cargo test` (all
tests in one process):

`plugin_help_does_not_run_an_unapproved_binary` (`dispatch_tests.rs`) sets
`PATH` to a temp dir holding only `jp-titles`, under `#[serial(env_vars)]`.
Tests that spawn `sleep` or `sh` by name, and aren't marked serial, fail while
it runs:

- `process_tree::tests::a_process_outside_the_tree_is_left_alone` and
  `terminating_the_tree_kills_what_the_plugin_started`: `spawn` returns
  `NotFound`.
- `dispatch::tests::stop_plugin_kills_a_plugin_and_its_workers`: its `#!/bin/sh`
  script can't find `sleep`, so `wait` returns at once and the plugin exits 0
  instead of being killed.

nextest runs each test in its own process, so CI and `just coverage` don't see
this.
Note that a guard-owned lock alone wouldn't fix it either: these tests only
*read* `PATH` (by spawning), so they hold no guard.
That's the "tests that only read the environment" question above.
