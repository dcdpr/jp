//! Self-measuring spike for the multi-row status region's draw and erase
//! sequence (RFD 091 phase 4).
//!
//! `Printer::memory` records the bytes JP emits and models nothing a terminal
//! does with them, so scrolling, deferred wrap, and resize can only be settled
//! against a real one.
//! Rather than ask an operator to watch, each case queries the terminal for the
//! cursor position around every step and checks it against what the region's
//! own row accounting predicted.
//!
//! ```sh
//! cargo run -p jp_printer --example region_spike     # run and print the table
//! cargo run -p jp_printer --example region_spike -- --bytes
//! ```
//!
//! Runs unattended in about fifteen seconds and ends with a table to paste
//! back.
//! Delete this example once the PTY harness (ticket T-0994dfa) can assert the
//! same cases against a modelled screen.

use std::{
    env,
    io::{self, IsTerminal as _, Write},
    thread,
    time::{Duration, Instant},
};

/// Write `text`, discarding I/O errors: a spike has nowhere to report them and
/// nothing useful to do about them.
fn emit(out: &mut impl Write, text: &str) {
    let _err = out.write_all(text.as_bytes());
}

/// Write `text` followed by a line break.
fn line(out: &mut impl Write, text: &str) {
    emit(out, text);
    emit(out, "\n");
}

/// Push everything written so far to the terminal.
fn flush(out: &mut impl Write) {
    let _err = out.flush();
}

/// The terminal's `(columns, rows)`, falling back to a conservative guess.
fn size() -> (u16, u16) {
    crossterm::terminal::size().unwrap_or((80, 24))
}

/// The cursor's `(column, row)`, zero-based, or `(0, 0)` if the terminal will
/// not say.
fn cursor() -> (u16, u16) {
    crossterm::cursor::position().unwrap_or((0, 0))
}

/// How many line breaks a region emits before painting, to make sure the rows
/// it is about to fill exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// None: paint where the cursor already sits and let each row break scroll
    /// as it goes.
    Naive,

    /// One per row, as RFD 091 words it.
    ReserveAll,

    /// One per row *below the first*, since the first row is the one the cursor
    /// already occupies.
    ReserveBelow,
}

impl Mode {
    /// The label used in headings and the results table.
    const fn label(self) -> &'static str {
        match self {
            Self::Naive => "naive",
            Self::ReserveAll => "reserve-all",
            Self::ReserveBelow => "reserve-below",
        }
    }

    /// Line breaks to emit before the first paint of a `rows`-tall region.
    const fn reserve(self, rows: usize) -> usize {
        match self {
            Self::Naive => 0,
            Self::ReserveAll => rows,
            Self::ReserveBelow => rows.saturating_sub(1),
        }
    }
}

/// A block of terminal rows the spike paints and erases.
struct Region {
    /// How the first paint reserves its rows.
    mode: Mode,

    /// Physical rows currently painted; `0` when nothing is on screen.
    drawn: usize,
}

impl Region {
    /// A region that has not painted yet.
    const fn new(mode: Mode) -> Self {
        Self { mode, drawn: 0 }
    }

    /// Paint `rows`, reserving space on the first paint and reusing the rows
    /// already on screen for every paint after it.
    fn paint(&mut self, out: &mut impl Write, rows: &[String]) {
        if self.drawn == 0 {
            let reserve = self.mode.reserve(rows.len());
            for _ in 0..reserve {
                emit(out, "\n");
            }
            if reserve > 0 {
                emit(out, &format!("\x1b[{reserve}A"));
            }
        } else {
            self.rewind(out);
        }

        for (index, row) in rows.iter().enumerate() {
            emit(out, "\r\x1b[K");
            emit(out, row);
            if index + 1 < rows.len() {
                emit(out, "\n");
            }
        }

        flush(out);
        self.drawn = rows.len();
    }

    /// Clear every painted row, leaving the cursor at column 0 of the row the
    /// region began on.
    fn erase(&mut self, out: &mut impl Write) {
        if self.drawn == 0 {
            return;
        }

        self.rewind(out);
        flush(out);
        self.drawn = 0;
    }

    /// Walk from the last painted row back to the first, clearing each.
    fn rewind(&self, out: &mut impl Write) {
        emit(out, "\r\x1b[K");
        for _ in 1..self.drawn {
            emit(out, "\x1b[1A\r\x1b[K");
        }
    }
}

/// Build a region's rows: window rows above, status row last.
fn rows(count: usize, tick: usize) -> Vec<String> {
    let mut rows: Vec<String> = (1..count)
        .map(|n| format!("[{n}/{}] window row", count - 1))
        .collect();
    rows.push(format!("* status row - tick {tick}"));

    rows
}

/// Write `count` numbered content lines, each terminated so the cursor lands on
/// a fresh row.
fn content(out: &mut impl Write, prefix: &str, count: usize) {
    for n in 1..=count {
        line(out, &format!("{prefix} {n:02} ...................."));
    }
    flush(out);
}

/// Put the cursor on the last row of the terminal with four content lines
/// directly above it.
fn fill_to_bottom(out: &mut impl Write) {
    for _ in 0..size().1 {
        emit(out, "\n");
    }
    content(out, "content", 4);
}

/// One measured claim/paint/erase cycle.
struct Measured {
    /// Rows the region meant to occupy.
    rows: usize,

    /// Cursor before the first paint.
    before: (u16, u16),

    /// Cursor once the region is fully painted.
    painted: (u16, u16),

    /// Cursor once the region is erased.
    erased: (u16, u16),

    /// Terminal height when the region was painted.
    height_painted: u16,

    /// Terminal height when the region was erased.
    height_erased: u16,
}

impl Measured {
    /// Physical rows the region actually spanned, counted from the cursor.
    const fn span(&self) -> i32 {
        self.painted.1 as i32 - self.erased.1 as i32 + 1
    }

    /// Rows the erase actually walked back up.
    const fn walked(&self) -> i32 {
        self.painted.1 as i32 - self.erased.1 as i32
    }
}

/// A named pass/fail observation drawn from a [`Measured`] cycle.
struct Check {
    /// What is being checked.
    name: &'static str,

    /// Whether the terminal agreed with the region's accounting.
    ok: bool,

    /// What was seen, whether or not it passed.
    detail: String,
}

/// The region spanned exactly the rows it painted.
fn check_span(m: &Measured) -> Check {
    let span = m.span();
    let want = i32::try_from(m.rows).unwrap_or(i32::MAX);

    Check {
        name: "span",
        ok: span == want,
        detail: format!("{span} of {want} rows"),
    }
}

/// The erase left the cursor at column 0.
fn check_column(m: &Measured) -> Check {
    Check {
        name: "column",
        ok: m.erased.0 == 0,
        detail: format!("col {}", m.erased.0),
    }
}

/// The region's last row was the terminal's last row.
fn check_anchored(m: &Measured) -> Check {
    let want = m.height_painted.saturating_sub(1);

    Check {
        name: "anchored",
        ok: m.painted.1 == want,
        detail: format!("last row {} of {want}", m.painted.1),
    }
}

/// The cursor came back to where the region was claimed, allowing for the rows
/// the terminal had to scroll out from under it to make room.
///
/// A region claimed with `room` rows to spare below the cursor scrolls `rows -
/// 1 - room` rows, and no more.
/// Scrolling further means the draw reserved space it did not need.
fn check_restored(m: &Measured) -> Check {
    let room = i32::from(m.height_painted) - 1 - i32::from(m.before.1);
    let scrolled = (i32::try_from(m.rows).unwrap_or(0) - 1 - room).max(0);
    let want = i32::from(m.before.1) - scrolled;
    let got = i32::from(m.erased.1);

    Check {
        name: "restored",
        ok: got == want && m.erased.0 == m.before.0,
        detail: format!("row {got}, want {want} (scrolled {scrolled}, room {room})"),
    }
}

/// The erase walked as far up as it meant to.
fn check_walked(m: &Measured) -> Check {
    let want = i32::try_from(m.rows.saturating_sub(1)).unwrap_or(i32::MAX);

    Check {
        name: "walked",
        ok: m.walked() == want,
        detail: format!(
            "{} of {want} rows, height {} -> {}",
            m.walked(),
            m.height_painted,
            m.height_erased
        ),
    }
}

/// Paint a region, tick it, erase it, and record what the terminal did.
fn cycle(out: &mut impl Write, mode: Mode, count: usize, ticks: usize) -> Measured {
    let before = cursor();
    let mut region = Region::new(mode);

    region.paint(out, &rows(count, 0));
    let height_painted = size().1;
    let painted = cursor();

    for tick in 1..=ticks {
        region.paint(out, &rows(count, tick));
        thread::sleep(Duration::from_millis(120));
    }

    let height_erased = size().1;
    region.erase(out);
    let erased = cursor();

    Measured {
        rows: count,
        before,
        painted,
        erased,
        height_painted,
        height_erased,
    }
}

/// Ask the terminal to resize itself, returning whether it obliged.
///
/// `\x1b[8;{rows};{columns}t` is XTWINOPS; plenty of terminals disable it,
/// which the poll below detects.
fn resize(out: &mut impl Write, columns: u16, height: u16) -> bool {
    emit(out, &format!("\x1b[8;{height};{columns}t"));
    flush(out);

    let deadline = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < deadline {
        if size().1 == height {
            // Give the terminal a moment to settle after the reflow.
            thread::sleep(Duration::from_millis(150));
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }

    false
}

/// Wait for the operator to shrink the window by hand, for terminals that
/// refuse XTWINOPS.
///
/// Polls the size rather than reading a keypress: stdin carries the cursor
/// replies, and a stray newline in it would desynchronise every later
/// measurement.
fn await_manual_resize(out: &mut impl Write, below: u16) -> bool {
    emit(
        out,
        &format!("\r\x1b[K\x1b[1mdrag this window shorter than {below} rows\x1b[0m"),
    );
    flush(out);

    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if size().1 < below {
            thread::sleep(Duration::from_millis(250));
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }

    false
}

/// One numbered case: what it does and what it is called in the table.
struct Case {
    /// The number the table refers to.
    number: usize,

    /// One-line description.
    title: String,

    /// The observations it produced.
    checks: Vec<Check>,
}

/// Run every case, returning the table rows.
fn run_all(out: &mut impl Write) -> Vec<Case> {
    let mut cases = Vec::new();
    let mut number = 0;
    let mut next = || {
        number += 1;
        number
    };

    // Mid-screen: plenty of room below, so nothing should scroll and the cursor
    // must land back exactly where it started.
    for mode in [Mode::Naive, Mode::ReserveAll, Mode::ReserveBelow] {
        content(out, "content", 4);
        let m = cycle(out, mode, 4, 3);
        cases.push(Case {
            number: next(),
            title: format!("{} - mid-screen - 4 rows", mode.label()),
            checks: vec![
                check_span(&m),
                check_column(&m),
                check_restored(&m),
                check_walked(&m),
            ],
        });
        content(out, "after", 1);
    }

    // Cursor on the last row: the region has to scroll its rows into existence
    // and must end flush against the bottom.
    for mode in [Mode::Naive, Mode::ReserveAll, Mode::ReserveBelow] {
        fill_to_bottom(out);
        let m = cycle(out, mode, 4, 3);
        cases.push(Case {
            number: next(),
            title: format!("{} - bottom - 4 rows", mode.label()),
            checks: vec![
                check_span(&m),
                check_column(&m),
                check_anchored(&m),
                check_restored(&m),
                check_walked(&m),
            ],
        });
        content(out, "after", 1);
    }

    // One row at the bottom: the shape phases 1-3 already shipped.
    for mode in [Mode::Naive, Mode::ReserveBelow] {
        fill_to_bottom(out);
        let m = cycle(out, mode, 1, 3);
        cases.push(Case {
            number: next(),
            title: format!("{} - bottom - 1 row", mode.label()),
            checks: vec![check_span(&m), check_column(&m), check_anchored(&m)],
        });
        content(out, "after", 1);
    }

    cases.push(persistent_write_case(out, next()));
    cases.push(full_width_case(out, next()));
    cases.push(shrink_case(out, next()));

    cases
}

/// Land three persistent writes while the region is on screen, checking that
/// the region survives each erase/write/paint round intact.
fn persistent_write_case(out: &mut impl Write, number: usize) -> Case {
    content(out, "content", 4);

    let mut region = Region::new(Mode::ReserveBelow);
    region.paint(out, &rows(4, 0));
    let painted = cursor();

    let mut spans = Vec::new();
    for n in 1..=3 {
        region.erase(out);
        let erased = cursor();
        spans.push(i32::from(painted.1) - i32::from(erased.1) + 1);

        line(out, &format!("written {n:02} ...................."));
        flush(out);
        region.paint(out, &rows(4, n));
        thread::sleep(Duration::from_millis(120));
    }

    region.erase(out);
    let final_erased = cursor();
    content(out, "after", 1);

    let consistent = spans.iter().all(|span| *span == 4);
    Case {
        number,
        title: "reserve-below - persistent writes while drawn".to_owned(),
        checks: vec![
            Check {
                name: "span",
                ok: consistent,
                detail: format!("{spans:?} across three writes, want [4, 4, 4]"),
            },
            Check {
                name: "column",
                ok: final_erased.0 == 0,
                detail: format!("col {}", final_erased.0),
            },
        ],
    }
}

/// Draw a region whose middle row is exactly as wide as the terminal; a
/// deferred-wrap terminal turns it into two physical rows and the span check
/// catches it.
fn full_width_case(out: &mut impl Write, number: usize) -> Case {
    let width = usize::from(size().0);
    content(out, "content", 4);

    let before = cursor();
    let mut region = Region::new(Mode::ReserveBelow);
    region.paint(out, &[
        format!(
            "{:<w$}",
            "[1/3] one column short",
            w = width.saturating_sub(1)
        ),
        format!("{:<w$}", "[2/3] exactly the full width", w = width),
        "[3/3] short row".to_owned(),
        "* status row".to_owned(),
    ]);
    let painted = cursor();
    thread::sleep(Duration::from_millis(400));

    let height = size().1;
    region.erase(out);
    let erased = cursor();
    content(out, "after", 1);

    let m = Measured {
        rows: 4,
        before,
        painted,
        erased,
        height_painted: height,
        height_erased: height,
    };

    Case {
        number,
        title: format!("reserve-below - row exactly {width} columns wide"),
        checks: vec![check_span(&m), check_column(&m), check_restored(&m)],
    }
}

/// Shrink the terminal below the drawn row count, then erase: the walk up
/// clamps at the top of the viewport and the check reports how far short it
/// stopped.
fn shrink_case(out: &mut impl Write, number: usize) -> Case {
    let (columns, original) = size();
    let count = usize::from(original).saturating_sub(4).max(4);

    content(out, "content", 2);

    let before = cursor();
    let mut region = Region::new(Mode::ReserveBelow);
    region.paint(out, &rows(count, 0));
    let painted = cursor();

    let target = u16::try_from(count / 2).unwrap_or(8).max(6);
    let rows_u16 = u16::try_from(count).unwrap_or(u16::MAX);
    let resized = resize(out, columns, target) || await_manual_resize(out, rows_u16);
    let height_erased = size().1;

    region.erase(out);
    let erased = cursor();

    content(out, "after", 1);

    let m = Measured {
        rows: count,
        before,
        painted,
        erased,
        height_painted: original,
        height_erased,
    };

    let title = if resized {
        format!("reserve-below - {count} rows, terminal shrunk to {height_erased}")
    } else {
        format!("reserve-below - {count} rows, NOT SHRUNK - result meaningless")
    };

    Case {
        number,
        title,
        checks: vec![check_walked(&m), check_span(&m), check_column(&m)],
    }
}

/// Print the escaped byte sequences each mode emits, without touching the
/// screen.
fn print_bytes(out: &mut impl Write) {
    line(out, "Sequences for a 4-row region:\n");

    for mode in [Mode::Naive, Mode::ReserveAll, Mode::ReserveBelow] {
        let mut buffer: Vec<u8> = Vec::new();
        let mut region = Region::new(mode);

        region.paint(&mut buffer, &rows(4, 0));
        let first = String::from_utf8_lossy(&buffer)
            .escape_default()
            .to_string();

        buffer.clear();
        region.paint(&mut buffer, &rows(4, 1));
        let again = String::from_utf8_lossy(&buffer)
            .escape_default()
            .to_string();

        buffer.clear();
        region.erase(&mut buffer);
        let erase = String::from_utf8_lossy(&buffer)
            .escape_default()
            .to_string();

        line(out, &format!("{}:", mode.label()));
        line(out, &format!("  first paint : {first}"));
        line(out, &format!("  repaint     : {again}"));
        line(out, &format!("  erase       : {erase}\n"));
    }

    flush(out);
}

/// Print the results table to paste back.
fn print_table(out: &mut impl Write, cases: &[Case], columns: u16, height: u16) {
    line(out, "\n=== region spike results ===");
    line(
        out,
        &format!("terminal {columns}x{height}, {}\n", env::consts::OS),
    );

    for case in cases {
        line(out, &format!("{}. {}", case.number, case.title));
        for check in &case.checks {
            let mark = if check.ok { "ok  " } else { "FAIL" };
            line(
                out,
                &format!("     {mark} {:<9} {}", check.name, check.detail),
            );
        }
    }

    let failed = cases
        .iter()
        .flat_map(|case| &case.checks)
        .filter(|check| !check.ok)
        .count();
    line(out, &format!("\n{failed} failed check(s)"));
    flush(out);
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut err = io::stderr();

    if args.iter().any(|arg| arg == "--bytes") {
        print_bytes(&mut io::stdout());
        return;
    }

    if !io::stderr().is_terminal() || !io::stdin().is_terminal() {
        line(
            &mut err,
            "stdin and stderr must both be terminals: the cases measure the cursor position.",
        );
        flush(&mut err);
        return;
    }

    let (columns, height) = size();
    line(
        &mut err,
        "\x1b[1mRFD 091 phase 4 spike\x1b[0m - runs unattended, resizes the window once, then \
         prints a table.",
    );

    let cases = run_all(&mut err);
    print_table(&mut io::stdout(), &cases, columns, height);
}
