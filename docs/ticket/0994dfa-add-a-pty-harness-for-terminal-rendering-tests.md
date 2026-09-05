# Add a PTY harness for terminal rendering tests

- **Status**: Done
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
the resulting byte sequence can only be pinned as a `Printer::memory` snapshot
— a test that asserts the bytes we chose, not that a terminal renders them
correctly.
The same gap covers the interactive prompts that motivated [issue 392]:
keystroke handling and widget redraw have no automated coverage either.

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
- Support resizing mid-test, so shrinking below a drawn row count is
  exercisable.
- Provide keystroke injection and a wait-for-screen-state helper; no fixed
  sleeps.
- Port the status region's multi-row draw and erase to screen-level assertions,
  replacing whichever byte snapshots phase 4 lands with.
- Cover the four cases phase 4's spike settles: claiming while the cursor sits
  on the last row, a persistent write landing while the region is drawn, the
  terminal shrinking below the drawn row count, and Windows.
- Skip cleanly rather than fail where a platform has no PTY.

Until this lands, phase 4's spike runs by hand: `cargo run -p jp_printer
--example region_spike` prints candidate sequences for a human to observe.
Delete the example when the harness replaces it.

## Comments

-----

- **From**: jp
- **Date**: 2026-09-05T19:40:11Z

Landed as `crates/jp_pty`, with `jp_printer`'s screen-level region cases ported
onto it and `examples/region_spike.rs` deleted.

## Library choice

**`portable-pty` over `expectrl`.** `expectrl` is built around spawning a child
and matching its output stream; it has no way to hand a writable tty to code
running in this process, which is what "drive `jp_printer` in-process" needs.
`portable-pty` hands back both ends and covers Windows through ConPTY, which
`expectrl` reaches only via a separate crate.

**`vt100` over `termwiz`.** Already in the tree, already vetted, and it answers
exactly the three questions the ticket asks for: what row `N` holds, where the
cursor is, and whether a row wrapped.
`termwiz` is a terminal toolkit; the screen model is a small part of it.

## Shape

`Terminal` has two backends behind one screen API, so a test body is written
once:

- `Terminal::pty(size)` — a real pty.
  Output crosses the kernel's line discipline, the writer is a tty, a child can
  be spawned into it and resized under it.
- `Terminal::modelled(size)` — the screen model fed directly, imitating
  `ONLCR`.
- `Terminal::open(size)` — the pty where the parent can write into one, the
  model otherwise.

The second backend is not a convenience.
Windows' ConPTY is reachable only by a child process, so a pty-only harness
would have made every ported region case unix-only — a *loss* of Windows
coverage against what phase 4 already had.
`a_pty_renders_what_the_model_renders` pins the two against each other on unix,
and fails if the model stops imitating a tty (checked by disabling the
translation).
`open_takes_the_pty_where_there_is_one` keeps the unix fallback from happening
silently.

`wait_for` blocks on a condvar woken by arriving bytes and returns the screen
that satisfied the predicate, so the assertions after it are made against that
snapshot rather than a later one.
Nothing sleeps for a fixed interval.
A timeout renders the screen with the cursor marked, and `Error`'s `Debug`
defers to `Display`, so an `unwrap` in a test prints it.

## Coverage against the four cases

| Case                                             | Where                                                                                                                 |
| ------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------- |
| Claiming with the cursor on the last row         | `a_block_claimed_at_the_bottom_ends_on_the_last_row`, `claiming_at_the_bottom_scrolls_content_up_rather_than_over_it` |
| A persistent write while the region is drawn     | `a_persistent_write_lands_above_the_block`                                                                            |
| The terminal shrinking below the drawn row count | `an_erase_after_a_shrink_stops_at_the_top_of_the_viewport`                                                            |
| Windows                                          | the whole `jp_printer` set, on the model backend; `jp_pty/tests/spawn.rs` on ConPTY                                   |

Three byte snapshots of multi-row draw and erase are gone, replaced by screen
assertions: `a_window_paints_its_lines_above_the_status_row`,
`a_shrinking_window_clears_the_rows_it_gives_back` (now
`a_window_that_shrinks_clears_the_rows_it_gives_back`), and
`an_erase_walks_no_further_than_the_viewport_has_rows`.

## Two things worth knowing

**The shrink case was asserting less than its name claimed.** It was
`assert!(term.cursor().0 < 4)`, which holds whether or not the erase is capped
— a cursor-up clamps at row 0 either way.
Strengthening it surfaced why: a resize is modelled as a truncation, and a
terminal reflows, so *which* rows survive a shrink is not something the harness
can answer.
The case is renamed to what it does pin — the erase clears every reachable row
and stops at the top of the viewport — and the comment says the rest is
unmodelled.
It now fails if the erase is removed.

**Nothing spawns `jp` yet.** The harness supports it and `tests/spawn.rs`
exercises the path end to end against a probe binary, but the interactive-prompt
tests from issue 392 are still to be written, and
`examples/mcp_window_fixture.toml` is still a by-hand run for RFD 091 phase 5.

RFD 091 phase 4 still reads "JP has no PTY harness"; left alone rather than
editing an accepted design record on a chore.

New third-party crates, all reachable only through dev-dependencies and exempted
at `safe-to-run`: `portable-pty`, `filedescriptor`, `nix`, `downcast-rs`,
`shell-words`, `serial2`, `shared_library`, `winreg`, `bitflags` 1.x,
`cfg_aliases`.
`cargo vet prune` will say if any are redundant against the imported audits.

[RFD 091]: https://jp.computer/rfd/091
[issue 392]: https://github.com/dcdpr/jp/issues/392
