use std::io::Write as _;

use jp_pty::{Screen, Size, Terminal, Writer};

use super::*;
use crate::printer::OutputFormat;

/// A terminal of unknown size: rows are neither truncated nor windowed.
fn unsized_terminal() -> TerminalCapability {
    TerminalCapability::interactive(None)
}

/// A stack holding one immediately-visible region, plus its id.
fn visible_stack(terminal: TerminalCapability) -> (RegionStack, RegionId, Vec<u8>) {
    let mut stack = RegionStack::new();
    let mut out = Vec::new();
    let style = RegionStyle::new(Duration::ZERO, Duration::from_millis(50), |_, detail| {
        detail.unwrap_or("waiting").to_owned()
    });

    stack.claim_test(1, style, terminal, &mut out);
    (stack, 1, out)
}

#[test]
fn regions_need_a_pretty_format() {
    let capability = TerminalCapability::interactive(Some(80));

    assert!(capability.permits_regions(OutputFormat::TextPretty));
    assert!(!capability.permits_regions(OutputFormat::Text));
    assert!(!capability.permits_regions(OutputFormat::Json));
    assert!(!capability.permits_regions(OutputFormat::JsonPretty));
}

#[test]
fn regions_need_an_interactive_stderr() {
    assert!(!TerminalCapability::default().permits_regions(OutputFormat::TextPretty));
}

#[test]
fn regions_are_off_while_logs_go_to_stderr() {
    let capability = TerminalCapability::interactive(Some(80)).with_stderr_logging(true);

    assert!(!capability.permits_regions(OutputFormat::TextPretty));
}

#[test]
fn claiming_a_due_region_paints_it_immediately() {
    let (_stack, _id, out) = visible_stack(unsized_terminal());

    assert_eq!(String::from_utf8(out).unwrap(), "\r\x1b[Kwaiting");
}

#[test]
fn a_delayed_region_paints_nothing_until_its_delay_passes() {
    let mut stack = RegionStack::new();
    let mut out = Vec::new();
    let style = RegionStyle::new(Duration::from_mins(1), Duration::from_millis(50), |_, _| {
        "waiting".to_owned()
    });

    stack.claim_test(1, style, unsized_terminal(), &mut out);
    stack.redraw(&mut out);

    assert!(out.is_empty());
}

#[test]
fn releasing_the_top_region_erases_its_row() {
    let (mut stack, id, mut out) = visible_stack(unsized_terminal());
    out.clear();

    stack.release(id, &mut out);

    assert_eq!(String::from_utf8(out).unwrap(), "\r\x1b[K");
}

#[test]
fn setting_a_detail_repaints_the_row() {
    let (mut stack, id, mut out) = visible_stack(unsized_terminal());
    out.clear();

    stack.set_detail(id, "starting bookworm".to_owned(), &mut out);

    assert_eq!(String::from_utf8(out).unwrap(), "\r\x1b[Kstarting bookworm");
}

#[test]
fn a_background_wraps_the_row_and_its_erase() {
    let (mut stack, id, mut out) = visible_stack(unsized_terminal());
    out.clear();

    stack.set_background(id, Some("\x1b[48;5;236m".to_owned()), &mut out);
    // The erase asserts the background too: `\x1b[K` fills with whatever is
    // active, so a region inside a shaded block must not punch a hole in it.
    stack.erase(&mut out);

    assert_eq!(
        String::from_utf8(out).unwrap(),
        "\r\x1b[48;5;236m\x1b[Kwaiting\x1b[49m\r\x1b[48;5;236m\x1b[K\x1b[49m"
    );
}

#[test]
fn the_newest_claim_renders_and_release_re_exposes_the_one_below() {
    let (mut stack, first, mut out) = visible_stack(unsized_terminal());

    let style = RegionStyle::new(Duration::ZERO, Duration::from_millis(50), |_, _| {
        "running tool".to_owned()
    });
    stack.claim_test(2, style, unsized_terminal(), &mut out);
    out.clear();

    stack.release(2, &mut out);

    assert_eq!(
        String::from_utf8(out).unwrap(),
        // Erase the tool row, then repaint the region underneath it.
        "\r\x1b[K\r\x1b[Kwaiting"
    );
    assert_eq!(first, 1);
}

#[test]
fn releasing_a_buried_claim_leaves_the_screen_alone() {
    let (mut stack, buried, mut out) = visible_stack(unsized_terminal());

    let style = RegionStyle::new(Duration::ZERO, Duration::from_millis(50), |_, _| {
        "running tool".to_owned()
    });
    stack.claim_test(2, style, unsized_terminal(), &mut out);
    out.clear();

    stack.release(buried, &mut out);

    assert!(out.is_empty());
}

#[test]
fn suspension_erases_the_row_and_blocks_redraws() {
    let (mut stack, id, mut out) = visible_stack(unsized_terminal());
    out.clear();

    stack.suspend(&mut out);
    assert_eq!(String::from_utf8(out.clone()).unwrap(), "\r\x1b[K");
    assert!(
        stack.tick_after().is_none(),
        "a suspended region must not tick"
    );

    // Nothing lands while suspended, including a detail update.
    out.clear();
    stack.redraw(&mut out);
    stack.set_detail(id, "still here".to_owned(), &mut out);
    assert!(out.is_empty());

    stack.resume(&mut out);
    assert_eq!(String::from_utf8(out).unwrap(), "\r\x1b[Kstill here");
}

#[test]
fn nested_suspensions_resume_only_on_the_last_release() {
    let (mut stack, _id, mut out) = visible_stack(unsized_terminal());

    stack.suspend(&mut out);
    stack.suspend(&mut out);
    out.clear();

    stack.resume(&mut out);
    assert!(out.is_empty(), "one suspension is still held");

    stack.resume(&mut out);
    assert_eq!(String::from_utf8(out).unwrap(), "\r\x1b[Kwaiting");
}

#[test]
fn an_empty_stack_never_ticks() {
    assert!(RegionStack::new().tick_after().is_none());
}

#[test]
fn a_pending_region_ticks_at_its_remaining_delay() {
    let mut stack = RegionStack::new();
    let mut out = Vec::new();
    let style = RegionStyle::new(Duration::from_mins(1), Duration::from_millis(50), |_, _| {
        String::new()
    });

    stack.claim_test(1, style, unsized_terminal(), &mut out);

    let tick = stack.tick_after().expect("a claimed region ticks");
    assert!(
        tick > Duration::from_secs(59) && tick <= Duration::from_mins(1),
        "a region waiting out its delay wakes when the delay ends, not on its interval; got \
         {tick:?}"
    );
}

#[test]
fn a_visible_region_ticks_at_its_interval() {
    let (stack, _id, _out) = visible_stack(unsized_terminal());

    assert_eq!(stack.tick_after(), Some(Duration::from_millis(50)));
}

#[test]
fn intervals_below_the_floor_are_raised() {
    let style = RegionStyle::new(Duration::ZERO, Duration::ZERO, |_, _| String::new());

    assert_eq!(style.interval, MIN_INTERVAL);
}

#[test]
fn rows_are_bounded_to_the_captured_column_count() {
    let mut stack = RegionStack::new();
    let mut out = Vec::new();
    let style = RegionStyle::new(Duration::ZERO, Duration::from_millis(50), |_, _| {
        "0123456789".to_owned()
    });

    stack.claim_test(1, style, TerminalCapability::interactive(Some(6)), &mut out);

    assert_eq!(String::from_utf8(out).unwrap(), "\r\x1b[K012345");
}

/// A region with a three-row window on a terminal tall enough to allow it.
fn windowed_stack(rows: u16) -> (RegionStack, RegionId, Vec<u8>) {
    let mut stack = RegionStack::new();
    let mut out = Vec::new();
    let style = RegionStyle::new(Duration::ZERO, Duration::from_millis(50), |_, _| {
        "* status".to_owned()
    })
    .with_output(OutputLines::Rows(3));

    let terminal = TerminalCapability::interactive(None).with_rows(Some(rows));
    stack.claim_test(1, style, terminal, &mut out);
    out.clear();

    (stack, 1, out)
}

#[test]
fn a_full_window_drops_its_oldest_line() {
    let (mut stack, id, mut out) = windowed_stack(40);

    for n in 1..=5 {
        stack.push(id, Arc::from("build"), &format!("line {n}"));
    }
    out.clear();
    stack.redraw(&mut out);

    let frame = String::from_utf8(out).unwrap();
    assert!(!frame.contains("line 1"), "oldest lines evict: {frame:?}");
    assert!(!frame.contains("line 2"));
    for kept in ["line 3", "line 4", "line 5"] {
        assert!(
            frame.contains(kept),
            "{kept} should still be shown: {frame:?}"
        );
    }
}

#[test]
fn one_source_renders_verbatim() {
    let (mut stack, id, mut out) = windowed_stack(40);

    stack.push(id, Arc::from("bookworm"), "compiling serde");
    out.clear();
    stack.redraw(&mut out);

    let frame = String::from_utf8(out).unwrap();
    assert!(frame.contains("\r\x1b[Kcompiling serde"), "{frame:?}");
    assert!(
        !frame.contains("[bookworm]"),
        "no label for a single source: {frame:?}"
    );
}

#[test]
fn two_sources_label_every_line() {
    let (mut stack, id, mut out) = windowed_stack(40);

    stack.push(id, Arc::from("bookworm"), "compiling serde");
    stack.push(id, Arc::from("grizzly"), "compiling tantivy");
    out.clear();
    stack.redraw(&mut out);

    let frame = String::from_utf8(out).unwrap();
    // Padded to the widest label so the output lines up, and coloured so two
    // interleaved sources stay apart at a glance. Only the label is coloured;
    // the line keeps whatever styling the source gave it.
    assert!(
        frame.contains("\x1b[96m[bookworm]\x1b[39m compiling serde"),
        "{frame:?}"
    );
    assert!(
        frame.contains("\x1b[34m[grizzly ]\x1b[39m compiling tantivy"),
        "{frame:?}"
    );
}

#[test]
fn a_label_keeps_its_colour_across_runs() {
    // The point of hashing the name rather than counting sources: a server is
    // the same colour in every run, on every machine. Pinned so a change to the
    // hash or the palette has to be a deliberate one.
    assert_eq!(label_colour("bookworm"), 96);
    assert_eq!(label_colour("grizzly"), 34);
}

#[test]
fn two_labels_can_share_a_colour() {
    // Ten colours and a hash means collisions, and this pair is one: they cost
    // legibility when both are in the window at once, nothing more. Avoiding
    // them entirely would mean assigning by position, which is what makes a
    // colour change between runs.
    assert_eq!(label_colour("bookworm"), label_colour("kagi"));
}

#[test]
fn every_label_colour_comes_from_the_palette() {
    let long = "x".repeat(50);
    for name in ["", "a", "bb", "server-1", "server-2", "ééé", long.as_str()] {
        assert!(
            LABEL_COLOURS.contains(&label_colour(name)),
            "{name:?} produced a colour outside the palette"
        );
    }
}

#[test]
fn a_label_colour_closes_without_disturbing_the_row_background() {
    // `\x1b[0m` would clear the row background a reasoning region asserted
    // (RFD 095), so the label closes its foreground alone.
    let (mut stack, id, mut out) = windowed_stack(40);

    stack.set_background(id, Some("\x1b[48;5;236m".to_owned()), &mut out);
    stack.push(id, Arc::from("alpha"), "one");
    stack.push(id, Arc::from("beta"), "two");
    out.clear();
    stack.redraw(&mut out);

    let frame = String::from_utf8(out).unwrap();
    assert!(
        !frame.contains("\x1b[0m"),
        "a full reset would drop the row background: {frame:?}"
    );
    assert!(frame.contains("\x1b[39m"), "{frame:?}");
}

#[test]
fn a_label_survives_its_source_falling_out_of_the_window() {
    // Labelling keys on what is in the window, not on which sources are still
    // pushing: a finished server's output must not be handed to the one still
    // running.
    let (mut stack, id, mut out) = windowed_stack(40);

    stack.push(id, Arc::from("bookworm"), "compiling serde");
    stack.push(id, Arc::from("grizzly"), "compiling tantivy");
    stack.push(id, Arc::from("grizzly"), "compiling tantivy-query");
    out.clear();
    stack.redraw(&mut out);
    assert!(
        String::from_utf8(out.clone())
            .unwrap()
            .contains("[bookworm]"),
        "both sources are still in the window"
    );

    // Pushing past bookworm's only line leaves grizzly alone in the window.
    stack.push(id, Arc::from("grizzly"), "linking");
    out.clear();
    stack.redraw(&mut out);

    let frame = String::from_utf8(out).unwrap();
    assert!(
        !frame.contains("[grizzly"),
        "one source left, so no labels: {frame:?}"
    );
}

#[test]
fn an_unknown_height_leaves_the_region_a_bare_status_row() {
    let mut stack = RegionStack::new();
    let mut out = Vec::new();
    let style = RegionStyle::new(Duration::ZERO, Duration::from_millis(50), |_, _| {
        "* status".to_owned()
    })
    .with_output(OutputLines::Rows(3));

    stack.claim_test(1, style, unsized_terminal(), &mut out);
    stack.push(1, Arc::from("build"), "compiling serde");
    out.clear();
    stack.redraw(&mut out);

    assert_eq!(String::from_utf8(out).unwrap(), "\r\x1b[K* status");
}

#[test]
fn auto_takes_a_tenth_of_the_terminal() {
    assert_eq!(OutputLines::Auto.rows(Some(40)), 4);
    assert_eq!(OutputLines::Auto.rows(Some(24)), 2);
    assert_eq!(OutputLines::Off.rows(Some(40)), 0);
    assert_eq!(OutputLines::Rows(6).rows(Some(40)), 6);
}

#[test]
fn a_window_never_claims_the_whole_terminal() {
    // The status row plus a row of context stay free, so the erase always has
    // somewhere to walk back to.
    assert_eq!(OutputLines::Rows(100).rows(Some(10)), 8);
    assert_eq!(OutputLines::Rows(100).rows(Some(2)), 0);
    assert_eq!(OutputLines::Auto.rows(None), 0);
    assert_eq!(OutputLines::Rows(3).rows(None), 0);
}

#[test]
fn the_filter_keeps_styling_and_closes_it() {
    assert_eq!(filter_line("\x1b[31mred"), "\x1b[31mred\x1b[0m");
    assert_eq!(filter_line("plain"), "plain");
}

#[test]
fn the_filter_drops_everything_that_is_not_styling() {
    // A child emitting an erase or cursor movement would corrupt the worker's
    // own row accounting; one emitting `\x1b[2J` would wipe the screen.
    assert_eq!(filter_line("\x1b[2Jwiped"), "wiped");
    assert_eq!(filter_line("a\x1b[1Ab"), "ab");
    assert_eq!(filter_line("\x1b]0;title\x07text"), "text");
    assert_eq!(filter_line("tab\there"), "tabhere");
}

#[test]
fn the_filter_removes_conceal_but_keeps_its_neighbours() {
    // Text the reader cannot see has no place in a preview, but dropping the
    // whole sequence would take the bold and the colour with it.
    assert_eq!(filter_line("\x1b[1;8;31mx"), "\x1b[1;31mx\x1b[0m");
    assert_eq!(filter_line("\x1b[8mhidden"), "hidden");
}

// --- Screen-level cases -----------------------------------------------------
//
// A byte assertion says what JP emitted. It cannot say what the terminal did
// with it, and scrolling, deferred wrap, and cursor clamping only exist on the
// far side of that boundary — which is where multi-row erasure goes wrong. The
// cases below drive a terminal instead — a real pty where the platform has one
// — and assert on the rendered result.

/// A terminal `rows` tall and `columns` wide, and a handle to draw into it.
fn terminal(rows: u16, columns: u16) -> (Terminal, Writer) {
    let terminal = Terminal::open(Size::new(rows, columns));
    let writer = terminal.writer().expect("an open terminal is writable");

    (terminal, writer)
}

/// Write `count` numbered content lines, standing in for earlier output.
fn content(writer: &mut Writer, count: usize) {
    for n in 1..=count {
        writeln!(writer, "content {n:02}").unwrap();
    }
}

/// Leave the cursor on the last row with `count` content lines above it.
fn fill_to_bottom(writer: &mut Writer, rows: u16, count: usize) {
    for _ in 0..rows {
        writeln!(writer).unwrap();
    }
    content(writer, count);
}

/// Wait until the screen's last rows are exactly `rows`, and return it.
///
/// For a block anchored to the bottom of the screen.
fn wait_for_tail(terminal: &Terminal, rows: &[&str]) -> Screen {
    wait(
        terminal,
        &format!("the screen to end with {rows:?}"),
        |screen| screen.tail(rows.len()) == rows,
    )
}

/// Wait until every row with content on it is exactly `rows`, and return it.
///
/// For a block with blank screen below it, where [`wait_for_tail`] would have
/// to name the empty rows underneath.
fn wait_for_used(terminal: &Terminal, rows: &[&str]) -> Screen {
    wait(
        terminal,
        &format!("the screen to show {rows:?}"),
        |screen| screen.used() == rows,
    )
}

/// Wait for `predicate`, failing with the screen when it does not hold.
///
/// The wait carries the assertion: on a pty the region's bytes arrive on
/// another thread, so reading the screen without one can catch a half-drawn
/// frame.
fn wait(terminal: &Terminal, what: &str, predicate: impl Fn(&Screen) -> bool) -> Screen {
    match terminal.wait_for(what, predicate) {
        Ok(screen) => screen,
        Err(error) => panic!("{error}"),
    }
}

/// An empty stack, plus the style and capability for a region with `window`
/// output rows on a terminal of the given size.
fn windowed(
    window: u16,
    columns: u16,
    rows: u16,
) -> (RegionStack, RegionStyle, TerminalCapability) {
    let style = RegionStyle::new(Duration::ZERO, Duration::from_millis(50), |_, _| {
        "* status".to_owned()
    })
    .with_output(OutputLines::Rows(window));

    (
        RegionStack::new(),
        style,
        TerminalCapability::interactive(Some(columns)).with_rows(Some(rows)),
    )
}

#[test]
fn a_block_claimed_at_the_bottom_ends_on_the_last_row() {
    // The reserve step emits one line break per row *below* the cursor's own.
    // One per row leaves the block a row short of the bottom, which is what the
    // RFD's wording would have produced and what the terminal spike measured.
    let (term, mut tty) = terminal(8, 40);
    fill_to_bottom(&mut tty, 8, 3);

    let (mut stack, style, cap) = windowed(2, 40, 8);
    stack.claim_test(1, style, cap, &mut tty);
    stack.push(1, Arc::from("build"), "one");
    stack.push(1, Arc::from("build"), "two");
    stack.redraw(&mut tty);

    let screen = wait_for_tail(&term, &["content 03", "one", "two", "* status"]);
    assert_eq!(
        screen.cursor().0,
        7,
        "the block ends flush against the bottom"
    );
}

#[test]
fn claiming_at_the_bottom_scrolls_content_up_rather_than_over_it() {
    let (term, mut tty) = terminal(8, 40);
    fill_to_bottom(&mut tty, 8, 3);

    let (mut stack, style, cap) = windowed(2, 40, 8);
    stack.claim_test(1, style, cap, &mut tty);
    stack.push(1, Arc::from("build"), "one");
    stack.push(1, Arc::from("build"), "two");
    stack.redraw(&mut tty);

    let screen = wait_for_tail(&term, &["content 03", "one", "two", "* status"]);
    for line in ["content 01", "content 02"] {
        assert!(screen.contains(line), "{line} was overwritten:\n{screen}");
    }
}

#[test]
fn releasing_a_block_puts_the_screen_back() {
    let (term, mut tty) = terminal(10, 40);
    content(&mut tty, 3);
    let before = wait_for_used(&term, &["content 01", "content 02", "content 03"]);

    let (mut stack, style, cap) = windowed(3, 40, 10);
    stack.claim_test(1, style, cap, &mut tty);
    for line in ["one", "two", "three"] {
        stack.push(1, Arc::from("build"), line);
    }
    stack.redraw(&mut tty);
    wait_for_used(&term, &[
        "content 01",
        "content 02",
        "content 03",
        "one",
        "two",
        "three",
        "* status",
    ]);

    stack.release(1, &mut tty);

    // Rows and cursor both: a screen equal to the one before the claim is an
    // erase that left no trace and put the cursor back where it found it.
    wait(&term, "the screen the block was drawn over", |screen| {
        *screen == before
    });
}

#[test]
fn a_persistent_write_lands_above_the_block() {
    let (term, mut tty) = terminal(10, 40);
    content(&mut tty, 2);

    let (mut stack, style, cap) = windowed(2, 40, 10);
    stack.claim_test(1, style, cap, &mut tty);
    stack.push(1, Arc::from("build"), "one");
    stack.redraw(&mut tty);

    // What the worker does around a `Print`: erase, let the content land,
    // paint again.
    stack.erase(&mut tty);
    writeln!(tty, "written 01").unwrap();
    stack.redraw(&mut tty);

    wait_for_used(&term, &[
        "content 01",
        "content 02",
        "written 01",
        "one",
        "* status",
    ]);
}

#[test]
fn a_window_that_shrinks_clears_the_rows_it_gives_back() {
    let (term, mut tty) = terminal(10, 40);
    content(&mut tty, 2);

    let (mut stack, style, cap) = windowed(3, 40, 10);
    stack.claim_test(1, style, cap, &mut tty);
    stack.push(1, Arc::from("build"), "one");
    stack.push(1, Arc::from("build"), "two");
    stack.redraw(&mut tty);
    wait_for_used(&term, &[
        "content 01",
        "content 02",
        "one",
        "two",
        "* status",
    ]);

    // The window empties and the block drops to a bare status row. Nothing will
    // overwrite the two rows it gave back, so it has to clear them itself.
    stack.entries[0].buffer.lock().lines.clear();
    stack.redraw(&mut tty);

    let screen = wait_for_used(&term, &["content 01", "content 02", "* status"]);
    assert_eq!(screen.cursor().0, 2, "the cursor ends on the status row");
}

#[test]
fn an_erase_after_a_shrink_stops_at_the_top_of_the_viewport() {
    // A terminal shrunk below the drawn row count cannot reach the rows it lost;
    // the erase walks no further than the viewport has, clearing what it can and
    // leaving the rest.
    //
    // Which rows survive a shrink is the terminal's reflow policy — whether it
    // truncates from the bottom or scrolls the top into scrollback — and the
    // screen model imitates neither, so that half is not what this pins.
    let (term, mut tty) = terminal(12, 40);
    content(&mut tty, 2);

    let (mut stack, style, cap) = windowed(5, 40, 12);
    stack.claim_test(1, style, cap, &mut tty);
    for n in 1..=5 {
        stack.push(1, Arc::from("build"), &format!("line {n}"));
    }
    stack.redraw(&mut tty);
    wait_for_used(&term, &[
        "content 01",
        "content 02",
        "line 1",
        "line 2",
        "line 3",
        "line 4",
        "line 5",
        "* status",
    ]);
    assert_eq!(stack.drawn_rows, 6);

    // The user drags the window down to fewer rows than the block occupies.
    term.resize(Size::new(4, 40)).unwrap();
    stack.entries[0].terminal = TerminalCapability::interactive(Some(40)).with_rows(Some(4));

    stack.erase(&mut tty);

    let screen = wait(
        &term,
        "the erase to reach the top of the viewport",
        |screen| screen.cursor() == (0, 0),
    );
    assert!(
        screen.used().is_empty(),
        "every reachable row was cleared:\n{screen}"
    );
}

#[test]
fn a_row_exactly_the_terminal_width_does_not_wrap() {
    // A wrapped row is one physical row the erase does not know about, so the
    // boundary case has to stay unwrapped for the row accounting to hold.
    let (term, mut tty) = terminal(8, 20);
    content(&mut tty, 2);

    let (mut stack, style, cap) = windowed(1, 20, 8);
    stack.claim_test(1, style, cap, &mut tty);
    stack.push(1, Arc::from("build"), &"x".repeat(20));
    stack.redraw(&mut tty);

    let screen = wait_for_used(&term, &[
        "content 01",
        "content 02",
        &"x".repeat(20),
        "* status",
    ]);
    assert!(
        !screen.wrapped(2),
        "a full-width row must not spill onto the next"
    );
    assert_eq!(stack.drawn_rows, 2, "still two physical rows");
}

#[test]
fn an_over_wide_row_is_cut_to_the_width() {
    let (term, mut tty) = terminal(8, 20);
    content(&mut tty, 2);

    let (mut stack, style, cap) = windowed(1, 20, 8);
    stack.claim_test(1, style, cap, &mut tty);
    stack.push(1, Arc::from("build"), &"y".repeat(60));
    stack.redraw(&mut tty);

    let screen = wait_for_used(&term, &[
        "content 01",
        "content 02",
        &"y".repeat(20),
        "* status",
    ]);
    assert!(!screen.wrapped(2));
    assert_eq!(stack.drawn_rows, 2);
}

#[test]
fn truncate_row_leaves_a_fitting_row_alone() {
    assert_eq!(truncate_row("⏱ Waiting… 4.2s", 40), "⏱ Waiting… 4.2s");
}

#[test]
fn truncate_row_measures_visible_columns() {
    // The escapes cost no columns, so all ten digits survive a budget of ten.
    assert_eq!(
        truncate_row("\x1b[2m0123456789\x1b[22m", 10),
        "\x1b[2m0123456789\x1b[22m"
    );
}

#[test]
fn truncate_row_keeps_escapes_whole_and_drops_text() {
    // The cut falls inside the styled run: the opening and closing sequences
    // are copied verbatim so the terminal is never left mid-sequence, and the
    // reset still lands.
    assert_eq!(
        truncate_row("\x1b[2m0123456789\x1b[22m", 4),
        "\x1b[2m0123\x1b[22m"
    );
}

#[test]
fn truncate_row_cuts_on_grapheme_boundaries() {
    // A budget that lands mid-character drops the character rather than
    // splitting its bytes.
    assert_eq!(truncate_row("aé日", 2), "aé");
}

#[test]
fn escape_end_spans_a_csi_sequence() {
    assert_eq!(escape_end("\x1b[48;5;236mrest"), 11);
    assert_eq!(escape_end("\x1b[Krest"), 3);
}

#[test]
fn escape_end_spans_an_osc_string() {
    assert_eq!(escape_end("\x1b]8;;https://example.com\x07rest"), 25);
    assert_eq!(escape_end("\x1b]0;title\x1b\\rest"), 11);
}

#[test]
fn escape_end_runs_to_the_end_of_an_unterminated_sequence() {
    assert_eq!(escape_end("\x1b[48;5"), 6);
    assert_eq!(escape_end("\x1b"), 1);
}
