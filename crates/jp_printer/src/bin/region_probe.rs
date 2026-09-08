//! A program a pty test spawns to draw a status region into a real terminal.
//!
//! It exists because a spawned child is the only way into a pty on Windows, and
//! the region's multi-row draw and erase are the one part of `jp_printer` whose
//! correctness is the console's answer rather than JP's.
//! In-process tests reach a real pty on unix and the screen model everywhere
//! else, so without this the sequence never runs against a Windows console at
//! all.
//!
//! The step to run is an argument rather than a command on stdin, because a pty
//! echoes what is typed into it.
//! An echoed line lands at the cursor, which sits inside the block, and scrolls
//! the screen by a row the region never accounted for — leaving its top row
//! stranded and every later erase one row low.
//! The region is right and the measurement was wrong, which is a bad way to
//! spend an afternoon.
//!
//! - `draw` — claim a region and hold it until killed
//! - `release` — claim a region, release it, then write a line over where it
//!   was

use std::{env, io, process, time::Duration};

use jp_printer::{OutputFormat, OutputLines, Printer, RegionStyle};

/// Content rows printed before the region is claimed.
///
/// More than a test opens the terminal with, so the cursor is on the last row
/// by the time the region is claimed.
/// That is the case worth measuring: the block has to scroll the screen to make
/// room rather than drawing into rows that already exist, and a console that
/// scrolls differently from a VT terminal shows up here or nowhere.
const CONTENT_ROWS: usize = 30;

/// Window rows the region shows above its status row.
const WINDOW_ROWS: u16 = 2;

fn main() {
    let step = env::args().nth(1).unwrap_or_default();
    let printer = Printer::terminal(OutputFormat::TextPretty);

    for row in 1..=CONTENT_ROWS {
        printer.println(format!("content {row:02}"));
    }

    let mut region = printer.status_region(
        RegionStyle::new(Duration::ZERO, Duration::from_millis(50), |_, _| {
            "probe status".to_owned()
        })
        .with_output(OutputLines::Rows(WINDOW_ROWS)),
    );

    let sink = region.source("probe");
    sink.push("probe window one");
    sink.push("probe window two");
    printer.flush();

    match step.as_str() {
        // Hold the block on screen. The test reads it and kills the child.
        "draw" => loop {
            std::thread::sleep(Duration::from_secs(1));
        },
        "release" => {
            region.release();
            // A persistent write after the release, so the test syncs on it
            // appearing rather than on the block's absence, and sees whether it
            // landed where the block was or on top of what survived.
            printer.println("released");
            printer.flush();
            printer.shutdown();
        }
        other => {
            printer.println(format!("unknown step: {other}"));
            printer.flush();
            printer.shutdown();
            process::exit(2);
        }
    }

    drop(io::stdout());
}
