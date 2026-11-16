# PTY probe tests kill exiting children mid-profraw write

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-10
- **Label**: domain=tooling
- **Label**: package=jp_inquire
- **Label**: type=bug

`coverage-ci` intermittently fails in `cargo llvm-cov report` while merging
`.profraw` files:

```text
warning: .../target/llvm-cov-target/jp-17567-14766903636987875402_3.profraw: invalid instrumentation profile data (file header is corrupt)
error: no profile can be merged
```

`cargo llvm-cov nextest` sets `LLVM_PROFILE_FILE` to `jp-%p-%Nm.profraw` (`%4m`
on 4-core runners).
With `%Nm` pool merging enabled, LLVM's `compiler-rt` (`InstrProfilingFile.c`)
does not create the file at startup; instead, an `atexit` handler opens and
writes it via buffered `fwrite()` after `main()` returns.
`llvm-profdata merge` ignores 0-byte `.profraw` files, but its default
`-failure-mode=any` aborts the entire merge if even one `.profraw` file is
partially written.

A partial `.profraw` happens when an instrumented probe binary
(`CARGO_BIN_EXE_*`) returns from `main()`, starts flushing coverage counters in
`atexit`, and the parent test immediately drops `jp_pty::Child`, whose `Drop`
impl (`crates/jp_pty/src/terminal.rs`) unconditionally sends `SIGKILL`
(`self.child.kill()`):

- `crates/jp_inquire/tests/inline_reply_pty.rs`
  (`a_seeded_buffer_is_shown_without_redrawing_the_screen`, added in #1217):
  `inline_reply_probe` prints `outcome: Submit(...)` and returns from `main()`.
  As soon as the parent sees `"outcome:"` on the PTY screen, the test exits
  without calling `probe.wait_within(TIMEOUT)` and drops `_probe`, sending
  `SIGKILL` while `inline_reply_probe` is writing a multi-megabyte `.profraw`.
- `crates/jp_pty/tests/spawn.rs` (`keystrokes_reach_the_child` and
  `a_resize_reaches_a_running_child`): both bind `let Some((terminal, _child)) =
  spawn_probe(...)` without sending `"quit\r"` or calling
  `child.wait_within(TIMEOUT)`.
  Tuple fields drop in declaration order: `terminal` drops first (closing the
  PTY manager so `pty_probe` hits stdin EOF, returns from `main()`, and enters
  `atexit`), and `_child` drops microseconds later, sending `SIGKILL`.

By contrast, `spawn.rs`
(`a_child_is_given_the_size_the_terminal_was_opened_with`) and
`crates/jp_printer/tests/region_pty.rs` (`releasing_puts_the_screen_back`)
already call `child.wait_within(TIMEOUT)` before dropping `Child`.

- [x] Wait for `inline_reply_probe` to exit via `wait_within(TIMEOUT)` in
      `crates/jp_inquire/tests/inline_reply_pty.rs`
- [x] Quit and wait for `pty_probe` via `wait_within(TIMEOUT)` in
      `keystrokes_reach_the_child` and `a_resize_reaches_a_running_child` in
      `crates/jp_pty/tests/spawn.rs`
- [x] Audit remaining `CARGO_BIN_EXE_*` tests (such as `jp_process` `hold:` /
      `spawn:` cases) or pass `--failure-mode=all` to `cargo llvm-cov report` in
      `justfile` if intentionally `SIGKILL`ed subprocesses can coincide with
      `atexit`

## Comments

-----

- **From**: jp
- **Date**: 2026-11-11T06:29:15Z

Fixed the three tests: each waits for its probe to exit via
`wait_within(TIMEOUT)` (asserting `Some(true)`) before dropping `Child`, with
the two `spawn.rs` cases sending `quit\r` first.
Updated the `jp_pty::Child` doc comment to advise calling `wait_within` before
dropping a child that exits on its own.

Audit of the other `CARGO_BIN_EXE_*` tests:

- `jp_printer/tests/region_pty.rs`: the `draw` cases kill `region_probe` while
  it is still in its `sleep` loop, so it never reaches `atexit`.
  `releasing_puts_the_screen_back` already waits.
- `jp_process/tests/system.rs`: every killed probe is either in `sleep:30000` or
  exits via `_exit` (`interrupt-exits`), which skips `atexit`.
  Probes that exit on their own (the `hold:` sleeper and the `tick`/`spin`
  children) are never killed.
  `a_cancelled_group_is_stopped_after_its_leader_exited` kills the process group
  about 300 ms after the leader returns from `main`, so the leader must finish
  writing its profile within that window.
  A small probe's profile flushes in milliseconds, well within that window, so
  the test needs no change.

Left `cargo llvm-cov report` at the default `--failure-mode=any` so future tests
that kill an exiting child mid-flush still fail CI rather than being masked.
