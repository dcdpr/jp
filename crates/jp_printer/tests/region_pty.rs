//! The status region's draw and erase, against a real terminal.
//!
//! The in-process cases in `region_tests.rs` reach a pty on unix and the screen
//! model everywhere else, because a pty's subsidiary end cannot be written to
//! from this process on Windows.
//! These spawn a child instead, which is the one way into a ConPTY, so the
//! multi-row sequence is measured against a real Windows console here or
//! nowhere.
//!
//! What comes back from ConPTY is a repaint of the console's own screen rather
//! than the bytes the child wrote, which is what is being asked for: where the
//! block lands is the console's business, and that is the thing under test.
//! On a GitHub-hosted Windows runner nothing comes back at all, which
//! `jp_pty::spawn_is_observable` reports and every case here honours.
//!
//! Nothing is typed at the probe.
//! A pty echoes its input onto the screen, and an echoed line at a cursor
//! sitting inside the block scrolls the screen by a row the region never
//! accounted for.
//! The step to run is an argument instead.

use std::time::Duration;

use jp_pty::{Child, CommandBuilder, Screen, Size, Terminal, spawn_is_observable};

/// The probe binary, built by cargo alongside this test.
const PROBE: &str = env!("CARGO_BIN_EXE_region_probe");

/// How long a case waits on the child.
///
/// Longer than the harness default: this waits on process startup under
/// whatever else CI is running at the time.
const TIMEOUT: Duration = Duration::from_secs(20);

/// Rows the terminal is opened with.
///
/// Small enough that the probe's content scrolls and the cursor is on the last
/// row when the region is claimed.
const ROWS: u16 = 10;

/// The rows the probe's block occupies, top first.
const BLOCK: [&str; 3] = ["probe window one", "probe window two", "probe status"];

/// Whether the block is fully drawn against the bottom of the screen.
///
/// Every row of it: a predicate satisfied by part of the frame freezes a
/// snapshot the rest of the frame has not reached yet.
fn drawn_block(screen: &Screen) -> bool {
    screen.tail(BLOCK.len()) == BLOCK
}

/// Run `step` in a terminal and wait for `what` to hold, or `None` where a
/// spawned child's output cannot be seen.
///
/// The child is returned rather than dropped, because dropping it kills it —
/// which is how the `draw` step is ended.
#[expect(clippy::print_stderr, reason = "a case that does nothing has to say so")]
fn probe(
    step: &str,
    what: &str,
    predicate: impl Fn(&Screen) -> bool,
) -> Option<(Terminal, Child, Screen)> {
    if !spawn_is_observable() {
        eprintln!("skipped: a spawned child's output is unreachable on this host");
        return None;
    }

    let terminal = Terminal::pty(Size::new(ROWS, 40))
        .expect("a pty")
        .with_timeout(TIMEOUT);

    let mut command = CommandBuilder::new(PROBE);
    command.arg(step);
    let child = terminal.spawn(command).expect("the probe to start");

    let screen = terminal
        .wait_for(what, predicate)
        .expect("the probe to reach the step being measured");

    Some((terminal, child, screen))
}

#[test]
fn a_block_claimed_at_the_bottom_ends_on_the_last_row() {
    // The reserve step emits one line break per row *below* the cursor's own.
    // One per row leaves the block a row short of the bottom, which is what the
    // RFD's wording would have produced.
    let Some((_terminal, _child, screen)) = probe("draw", "the block to be drawn", drawn_block)
    else {
        return;
    };

    assert_eq!(
        screen.cursor().0,
        ROWS - 1,
        "the block ends flush against the bottom:\n{screen}"
    );
}

#[test]
fn claiming_at_the_bottom_scrolls_content_up_rather_than_over_it() {
    // The content is written before the block is claimed, so once the block is
    // drawn nothing more can put those rows back.
    let Some((_terminal, _child, screen)) = probe("draw", "the block to be drawn", drawn_block)
    else {
        return;
    };

    assert!(
        screen.contains("content 30"),
        "the last content row survived the claim:\n{screen}"
    );
}

#[test]
fn the_block_occupies_one_physical_row_each() {
    // A wrapped row is a physical row the erase does not know about, so a block
    // row that wrapped would leave the walk short by one.
    let Some((_terminal, _child, screen)) = probe("draw", "the block to be drawn", drawn_block)
    else {
        return;
    };

    for row in (ROWS - 3)..ROWS {
        assert!(
            !screen.wrapped(row),
            "row {row} wrapped, so the erase would miscount:\n{screen}"
        );
    }
}

#[test]
fn releasing_puts_the_screen_back() {
    // The probe writes `released` after releasing the block, so a screen
    // showing it has already been through the erase.
    let Some((_terminal, mut child, screen)) =
        probe("release", "the write that follows the release", |screen| {
            screen.contains("released")
        })
    else {
        return;
    };

    assert!(
        !screen.contains("probe status"),
        "the status row is erased:\n{screen}"
    );
    assert!(
        !screen.contains("probe window"),
        "the window rows are erased:\n{screen}"
    );
    assert!(
        screen.contains("content 30"),
        "the content above the block survived:\n{screen}"
    );

    assert_eq!(
        child.wait_within(TIMEOUT).expect("the exit check"),
        Some(true),
        "the probe exits successfully once it has released the block"
    );
}
