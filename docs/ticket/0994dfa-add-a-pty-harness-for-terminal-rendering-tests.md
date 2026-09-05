# Add a PTY harness for terminal rendering tests

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-08-27

`Printer::memory` records the bytes JP emits and models nothing about what a
terminal does with them.
Scrolling, deferred wrap at the right margin, and resize only exist on a real
terminal, so any rendering that depends on cursor position across more than one
row cannot be tested at all today.

This blocks the multi-row status region ([RFD 091] phase 4).
Its draw and erase sequence has to be settled by watching a human run it, and
the resulting byte sequence can only be pinned as a `Printer::memory` snapshot —
a test that asserts the bytes we chose, not that a terminal renders them
correctly.
The same gap covers the interactive prompts that motivated [issue 392]: keystroke
handling and widget redraw have no automated coverage either.

A harness that drives a PTY and models the screen closes both.
Instead of asserting on emitted bytes, a test asserts on the resulting screen:
which rows hold what, where the cursor is, and what survived an erase.

Acceptance criteria:

- Evaluate `portable-pty` against `expectrl` and pick one; pair it with a screen
  model (`vt100` or `termwiz`) that answers "what does row N contain" and "where
  is the cursor".
- Provide a harness that runs a closure against a PTY of a declared size, so a
  test can drive `jp_printer` in-process rather than spawning `jp`.
- Support spawning `jp` in the PTY too, for end-to-end prompt tests.
- Support resizing mid-test, so shrinking below a drawn row count is exercisable.
- Provide keystroke injection and a wait-for-screen-state helper; no fixed
  sleeps.
- Port the status region's multi-row draw and erase to screen-level assertions,
  replacing whichever byte snapshots phase 4 lands with.
- Cover the four cases phase 4's spike settles: claiming while the cursor sits on
  the last row, a persistent write landing while the region is drawn, the
  terminal shrinking below the drawn row count, and Windows.
- Skip cleanly rather than fail where a platform has no PTY.

Until this lands, phase 4's spike runs by hand: `cargo run -p jp_printer
--example region_spike` prints candidate sequences for a human to observe.
Delete the example when the harness replaces it.

[RFD 091]: https://jp.computer/rfd/091
[issue 392]: https://github.com/dcdpr/jp/issues/392
