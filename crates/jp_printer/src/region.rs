//! Ephemeral status regions, drawn and erased by the printer's worker thread.
//!
//! A **status region** is a block of chrome rows at the bottom of the terminal:
//! a status row carrying a subject, an elapsed time, and an optional detail.
//! The worker draws it, ticks it, and erases it before any printer-managed
//! write on any channel reaches the terminal, so a client never has to order
//! its own clear against its own output.
//!
//! [`Printer::status_region`] claims one and hands back a [`StatusRegion`] that
//! releases the claim on drop.
//! Claims form a stack: the most recent one is rendered, and releasing it
//! re-exposes the one below.
//!
//! Regions render only on an interactive terminal that the printer has to
//! itself — see [`TerminalCapability`].
//! When they don't, a claim yields an inert handle whose methods do nothing.
//!
//! [`Printer::status_region`]: crate::Printer::status_region

use std::{
    fmt,
    io::{self, IsTerminal as _},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::Sender,
    },
    time::{Duration, Instant},
};

use jp_term::width::{display_width, prefix_end_for_width};

use crate::printer::{Command, OutputFormat};

/// Identifies one claimed region.
pub type RegionId = u64;

/// Hands out a fresh [`RegionId`] per claim.
static NEXT_REGION_ID: AtomicU64 = AtomicU64::new(0);

/// Smallest redraw interval the worker honors.
///
/// Anything shorter spins the worker without the terminal showing more.
const MIN_INTERVAL: Duration = Duration::from_millis(10);

/// CSI erase-in-line: clear from the cursor to the end of the row.
const ERASE_LINE: &str = "\x1b[K";

/// SGR background reset, closing a row background so it never leaks below the
/// region.
const BACKGROUND_END: &str = "\x1b[49m";

/// Renders a status row from the seconds elapsed since the claim and the
/// region's current detail.
type RowFormat = Box<dyn Fn(f64, Option<&str>) -> String + Send>;

/// What the chrome channel (stderr) can do, captured when the printer is built.
///
/// A status region renders only when all three hold: the output format permits
/// terminal control, stderr is an interactive terminal, and no tracing layer
/// writes to stderr behind the printer's back.
///
/// [`Printer::terminal`] measures a real terminal; tests declare one with
/// [`Self::interactive`]; every other printer defaults to no terminal at all.
///
/// [`Printer::terminal`]: crate::Printer::terminal
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TerminalCapability {
    /// Whether stderr is an interactive terminal.
    interactive: bool,

    /// The terminal's column count, when it could be measured.
    columns: Option<u16>,

    /// Whether a tracing layer writes to stderr.
    stderr_logging: bool,
}

impl TerminalCapability {
    /// Measure the chrome channel: stderr's tty-ness and the terminal's width.
    ///
    /// Yields a non-interactive capability when stderr is piped or redirected.
    #[must_use]
    pub fn detect() -> Self {
        if !io::stderr().is_terminal() {
            return Self::default();
        }

        Self {
            interactive: true,
            columns: crossterm::terminal::size().ok().map(|(columns, _)| columns),
            stderr_logging: false,
        }
    }

    /// An interactive terminal `columns` wide, declared rather than measured.
    ///
    /// Pass `None` for `columns` to model a terminal whose width could not be
    /// determined; rows are then left unbounded.
    #[must_use]
    pub const fn interactive(columns: Option<u16>) -> Self {
        Self {
            interactive: true,
            columns,
            stderr_logging: false,
        }
    }

    /// Record whether a tracing layer writes to stderr.
    ///
    /// Live logs make stderr a persistent stream the printer does not own, and
    /// a region it cannot erase around is worse than no region: rendering stays
    /// off while this is set.
    #[must_use]
    pub const fn with_stderr_logging(mut self, active: bool) -> Self {
        self.stderr_logging = active;
        self
    }

    /// The terminal's column count, when it is known.
    #[must_use]
    pub const fn columns(self) -> Option<u16> {
        self.columns
    }

    /// Whether a status region may render against `format`.
    pub(crate) const fn permits_regions(self, format: OutputFormat) -> bool {
        format.is_pretty() && self.interactive && !self.stderr_logging
    }
}

/// How a status region renders and ticks.
pub struct RegionStyle {
    /// How long after the claim the region stays invisible.
    delay: Duration,

    /// How often the status row redraws once it is visible.
    interval: Duration,

    /// Renders the status row from elapsed seconds and the current detail.
    format: RowFormat,
}

impl RegionStyle {
    /// A region that appears `delay` after the claim and redraws every
    /// `interval`.
    ///
    /// `format` receives the seconds elapsed since the claim and the region's
    /// current detail, and returns the row's text.
    /// Return content only: the worker owns cursor control, width bounding, and
    /// the row background.
    ///
    /// Intervals below 10ms are raised to 10ms.
    #[must_use]
    pub fn new(
        delay: Duration,
        interval: Duration,
        format: impl Fn(f64, Option<&str>) -> String + Send + 'static,
    ) -> Self {
        Self {
            delay,
            interval: interval.max(MIN_INTERVAL),
            format: Box::new(format),
        }
    }
}

impl fmt::Debug for RegionStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegionStyle")
            .field("delay", &self.delay)
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}

/// A claimed region's identity and the channel that reaches the worker.
#[derive(Debug, Clone)]
struct RegionRef {
    /// The claim this handle refers to.
    id: RegionId,

    /// The printer's command channel.
    tx: Sender<Command>,
}

impl RegionRef {
    /// Enqueue a region command, dropping it if the worker is already gone.
    fn send(&self, command: RegionCommand) {
        drop(self.tx.send(Command::Region(command)));
    }
}

/// An owned claim on the printer's status region.
///
/// Dropping it releases the claim, erases the region's rows, and re-exposes
/// whichever claim sits below it.
/// The handle is not `Clone`: [`Self::detail`] and [`Self::background`] hand
/// out cloneable capabilities for the parts that are safe to share.
///
/// An inert handle — the one a claim returns when regions are disabled —
/// accepts every call and does nothing.
#[derive(Debug)]
pub struct StatusRegion {
    /// `None` for an inert handle.
    region: Option<RegionRef>,
}

impl StatusRegion {
    /// A handle over a live claim.
    pub(crate) const fn new(id: RegionId, tx: Sender<Command>) -> Self {
        Self {
            region: Some(RegionRef { id, tx }),
        }
    }

    /// A handle that renders nothing.
    pub(crate) const fn inert() -> Self {
        Self { region: None }
    }

    /// Whether this handle is backed by a live claim.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.region.is_some()
    }

    /// Replace the detail passed to the region's format closure.
    ///
    /// The row redraws immediately rather than waiting for the next tick.
    pub fn set_detail(&self, detail: impl Into<String>) {
        if let Some(region) = &self.region {
            region.send(RegionCommand::Detail {
                id: region.id,
                detail: detail.into(),
            });
        }
    }

    /// A cloneable capability for updating the region's detail.
    #[must_use]
    pub fn detail(&self) -> StatusDetail {
        StatusDetail {
            region: self.region.clone(),
        }
    }

    /// A cloneable capability for setting the region's row background.
    #[must_use]
    pub fn background(&self) -> RowBackground {
        RowBackground {
            region: self.region.clone(),
        }
    }
}

impl Drop for StatusRegion {
    fn drop(&mut self) {
        if let Some(region) = &self.region {
            region.send(RegionCommand::Release { id: region.id });
        }
    }
}

/// Updates the detail shown on a region's status row.
///
/// Split off a [`StatusRegion`] so several holders can retitle the row without
/// any of them being able to release the claim.
#[derive(Debug, Clone)]
pub struct StatusDetail {
    /// `None` when the region it came from is inert.
    region: Option<RegionRef>,
}

impl StatusDetail {
    /// Replace the detail passed to the region's format closure.
    ///
    /// The row redraws immediately rather than waiting for the next tick.
    pub fn set(&self, detail: impl Into<String>) {
        if let Some(region) = &self.region {
            region.send(RegionCommand::Detail {
                id: region.id,
                detail: detail.into(),
            });
        }
    }
}

/// Sets the background the worker draws a region's rows against.
///
/// The background is an opaque SGR parameter (`48;5;236`, say): the printer
/// asserts it before every row it draws — its own erases included — and
/// closes it at the row's end so it never paints below the region.
#[derive(Debug, Clone)]
pub struct RowBackground {
    /// `None` when the region it came from is inert.
    region: Option<RegionRef>,
}

impl RowBackground {
    /// Draw the region's rows against the SGR background `param`.
    pub fn set(&self, param: impl AsRef<str>) {
        self.send(Some(format!("\x1b[{}m", param.as_ref())));
    }

    /// Return to the terminal's default background.
    pub fn clear(&self) {
        self.send(None);
    }

    /// Push a background escape (or its absence) to the worker.
    fn send(&self, background: Option<String>) {
        if let Some(region) = &self.region {
            region.send(RegionCommand::Background {
                id: region.id,
                background,
            });
        }
    }
}

/// Suspends status-region rendering for as long as it is held.
///
/// The rows are erased on acquisition and no redraw lands until the guard
/// drops, so a writer that owns the cursor — a prompt widget, an external
/// editor — is never interrupted mid-sequence.
#[derive(Debug)]
pub struct SuspendGuard {
    /// The printer's command channel; `None` when there was nothing to suspend.
    tx: Option<Sender<Command>>,
}

impl SuspendGuard {
    /// A guard that holds no suspension.
    pub(crate) const fn inert() -> Self {
        Self { tx: None }
    }

    /// A guard whose drop releases one suspension on `tx`.
    pub(crate) const fn new(tx: Sender<Command>) -> Self {
        Self { tx: Some(tx) }
    }
}

impl Drop for SuspendGuard {
    fn drop(&mut self) {
        if let Some(tx) = &self.tx {
            drop(tx.send(Command::Region(RegionCommand::Resume)));
        }
    }
}

/// Allocate an identifier for a new claim.
pub fn next_region_id() -> RegionId {
    NEXT_REGION_ID.fetch_add(1, Ordering::Relaxed)
}

/// A change to the worker's status regions.
///
/// Travels inside the printer's command channel so region updates stay ordered
/// with the writes they have to be erased around.
#[derive(Debug)]
pub enum RegionCommand {
    /// Push a new claim onto the stack.
    Claim {
        /// The claim's identifier.
        id: RegionId,

        /// How the region renders and ticks.
        style: RegionStyle,

        /// Columns the rows are bounded to, when the width is known.
        columns: Option<u16>,
    },

    /// Replace a claim's status-row detail.
    Detail {
        /// The claim to update.
        id: RegionId,

        /// The new detail.
        detail: String,
    },

    /// Replace the background a claim's rows are drawn against.
    Background {
        /// The claim to update.
        id: RegionId,

        /// The SGR escape to assert per row, or `None` for the terminal
        /// default.
        background: Option<String>,
    },

    /// Drop a claim, re-exposing the one below it.
    Release {
        /// The claim to release.
        id: RegionId,
    },

    /// Erase the drawn rows and block redraws until the matching
    /// [`Self::Resume`].
    Suspend {
        /// Signalled once the suspension has been applied, for callers that
        /// need the rows gone before they return.
        ack: Option<Sender<()>>,
    },

    /// Release one suspension.
    Resume,
}

/// One claimed region as the worker sees it.
struct RegionEntry {
    /// The claim's identifier.
    id: RegionId,

    /// When the claim was processed, and therefore what the row's elapsed time
    /// counts from.
    claimed_at: Instant,

    /// How the row renders and ticks.
    style: RegionStyle,

    /// Columns the row is bounded to, when the terminal's width is known.
    columns: Option<u16>,

    /// The detail passed to the format closure.
    detail: Option<String>,

    /// The pre-rendered SGR background escape for the row, when one is set.
    background: Option<String>,
}

impl RegionEntry {
    /// Whether the claim's delay has passed and the row belongs on screen.
    fn is_visible(&self) -> bool {
        self.claimed_at.elapsed() >= self.style.delay
    }

    /// The row's text, bounded to the captured column count.
    fn row(&self) -> String {
        let secs = self.claimed_at.elapsed().as_secs_f64();
        let row = (self.style.format)(secs, self.detail.as_deref());

        match self.columns {
            Some(columns) => truncate_row(&row, usize::from(columns)),
            None => row,
        }
    }
}

/// The worker's stack of claimed regions and what it last painted.
///
/// Only the top entry renders.
/// Every method that changes what belongs on screen takes the chrome writer, so
/// the terminal and this state can never disagree about how many rows are
/// drawn.
pub struct RegionStack {
    /// Claims in acquisition order; the last entry is the one rendered.
    entries: Vec<RegionEntry>,

    /// Physical rows the worker last painted; `0` when nothing is on screen.
    drawn_rows: usize,

    /// Nesting depth of active suspensions.
    suspensions: usize,
}

impl RegionStack {
    /// An empty stack with nothing drawn.
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            drawn_rows: 0,
            suspensions: 0,
        }
    }

    /// How long the worker may block before the top region needs a redraw.
    ///
    /// `None` when nothing is claimed or rendering is suspended, in which case
    /// the worker blocks until the next command arrives.
    pub fn tick_after(&self) -> Option<Duration> {
        if self.suspensions > 0 {
            return None;
        }

        let entry = self.entries.last()?;
        let remaining = entry.style.delay.saturating_sub(entry.claimed_at.elapsed());

        Some(if remaining.is_zero() {
            entry.style.interval
        } else {
            remaining
        })
    }

    /// Apply one region command, repainting the terminal as needed.
    pub fn apply(&mut self, command: RegionCommand, writer: &mut dyn io::Write) {
        match command {
            RegionCommand::Claim { id, style, columns } => self.claim(id, style, columns, writer),
            RegionCommand::Detail { id, detail } => self.set_detail(id, detail, writer),
            RegionCommand::Background { id, background } => {
                self.set_background(id, background, writer);
            }
            RegionCommand::Release { id } => self.release(id, writer),
            RegionCommand::Suspend { ack } => {
                self.suspend(writer);
                if let Some(tx) = ack {
                    let _ = tx.send(());
                }
            }
            RegionCommand::Resume => self.resume(writer),
        }
    }

    /// Push a new claim onto the stack and paint it if it is already due.
    fn claim(
        &mut self,
        id: RegionId,
        style: RegionStyle,
        columns: Option<u16>,
        writer: &mut dyn io::Write,
    ) {
        self.erase(writer);
        self.entries.push(RegionEntry {
            id,
            claimed_at: Instant::now(),
            style,
            columns,
            detail: None,
            background: None,
        });
        self.redraw(writer);
    }

    /// Drop a claim, re-exposing the one below it.
    fn release(&mut self, id: RegionId, writer: &mut dyn io::Write) {
        let Some(index) = self.entries.iter().position(|entry| entry.id == id) else {
            return;
        };

        // Erasing before the entry leaves the stack keeps the erase under the
        // background that entry was drawn against.
        let was_top = index + 1 == self.entries.len();
        if was_top {
            self.erase(writer);
        }

        self.entries.remove(index);

        if was_top {
            self.redraw(writer);
        }
    }

    /// Replace a claim's detail.
    fn set_detail(&mut self, id: RegionId, detail: String, writer: &mut dyn io::Write) {
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) else {
            return;
        };

        entry.detail = Some(detail);
        self.redraw_if_top(id, writer);
    }

    /// Replace a claim's row background.
    fn set_background(
        &mut self,
        id: RegionId,
        background: Option<String>,
        writer: &mut dyn io::Write,
    ) {
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) else {
            return;
        };

        entry.background = background;
        self.redraw_if_top(id, writer);
    }

    /// Erase the rows and block redraws until the matching [`Self::resume`].
    fn suspend(&mut self, writer: &mut dyn io::Write) {
        self.suspensions += 1;
        self.erase(writer);
    }

    /// Release one suspension, repainting once the last one is gone.
    fn resume(&mut self, writer: &mut dyn io::Write) {
        self.suspensions = self.suspensions.saturating_sub(1);
        if self.suspensions == 0 {
            self.redraw(writer);
        }
    }

    /// Clear every row the worker painted.
    pub fn erase(&mut self, writer: &mut dyn io::Write) {
        if self.drawn_rows == 0 {
            return;
        }

        // `\x1b[K` fills with whatever background is active, so the region's
        // own erase re-asserts the row background before clearing (RFD 095).
        let background = self
            .entries
            .last()
            .and_then(|entry| entry.background.clone());
        let mut row = String::from("\r");
        if let Some(background) = &background {
            row.push_str(background);
        }
        row.push_str(ERASE_LINE);
        if background.is_some() {
            row.push_str(BACKGROUND_END);
        }

        write_frame(writer, &row);
        self.drawn_rows = 0;
    }

    /// Paint the top claim, or erase when nothing belongs on screen.
    pub fn redraw(&mut self, writer: &mut dyn io::Write) {
        if self.suspensions > 0 {
            self.erase(writer);
            return;
        }

        let Some(entry) = self.entries.last() else {
            self.erase(writer);
            return;
        };

        if !entry.is_visible() {
            self.erase(writer);
            return;
        }

        // A single row is self-erasing: `\r` returns to column 0 and `\x1b[K`
        // clears whatever the previous frame left there.
        let mut row = String::from("\r");
        if let Some(background) = &entry.background {
            row.push_str(background);
        }
        row.push_str(ERASE_LINE);
        row.push_str(&entry.row());
        if entry.background.is_some() {
            row.push_str(BACKGROUND_END);
        }

        write_frame(writer, &row);
        self.drawn_rows = 1;
    }

    /// Repaint only when `id` is the claim currently on screen.
    fn redraw_if_top(&mut self, id: RegionId, writer: &mut dyn io::Write) {
        if self.entries.last().is_some_and(|entry| entry.id == id) {
            self.redraw(writer);
        }
    }
}

/// Write a region frame, discarding I/O errors.
///
/// A chrome row that cannot be written is not worth failing a run over, and the
/// worker has no channel to report it on.
fn write_frame(writer: &mut dyn io::Write, frame: &str) {
    let _err = writer.write_all(frame.as_bytes());
    let _err = writer.flush();
}

/// Bound `row` to `columns` visible columns, keeping escape sequences whole.
///
/// Escapes cost no columns and are copied verbatim, so a styled row is cut on
/// its text rather than in the middle of a sequence, and a trailing reset
/// survives the cut.
/// A row that already fits is returned unchanged.
fn truncate_row(row: &str, columns: usize) -> String {
    if display_width(row) <= columns {
        return row.to_owned();
    }

    let mut out = String::with_capacity(row.len());
    let mut budget = columns;
    let mut rest = row;

    while !rest.is_empty() {
        let text_end = rest.find('\x1b').unwrap_or(rest.len());
        if text_end == 0 {
            let end = escape_end(rest);
            out.push_str(&rest[..end]);
            rest = &rest[end..];
            continue;
        }

        budget -= push_bounded(&mut out, &rest[..text_end], budget);
        rest = &rest[text_end..];
    }

    out
}

/// Append as much of `text` as fits in `budget` columns, returning the columns
/// it consumed.
fn push_bounded(out: &mut String, text: &str, budget: usize) -> usize {
    if budget == 0 {
        return 0;
    }

    let taken = &text[..prefix_end_for_width(text, budget)];
    out.push_str(taken);
    display_width(taken)
}

/// Byte offset just past the escape sequence at the start of `s`.
///
/// A CSI sequence ends at its final byte (`0x40`–`0x7e`) and an OSC string at
/// BEL or ST; anything else is treated as a two-byte escape.
/// A sequence that never terminates runs to the end of the row.
fn escape_end(s: &str) -> usize {
    let bytes = s.as_bytes();
    let Some(&introducer) = bytes.get(1) else {
        return s.len();
    };

    match introducer {
        b'[' => bytes
            .iter()
            .enumerate()
            .skip(2)
            .find(|(_, byte)| (0x40..=0x7e).contains(*byte))
            .map_or(s.len(), |(at, _)| at + 1),
        b']' => osc_end(s),
        // A malformed escape followed by non-ASCII: consume the escape byte
        // alone rather than slicing into a multi-byte character.
        _ if s.is_char_boundary(2) => 2,
        _ => 1,
    }
}

/// Byte offset just past the OSC string starting at `s`, or `s.len()` when it
/// never terminates.
fn osc_end(s: &str) -> usize {
    let bytes = s.as_bytes();

    for (at, &byte) in bytes.iter().enumerate().skip(2) {
        if byte == 0x07 {
            return at + 1;
        }
        if byte == 0x1b && bytes.get(at + 1) == Some(&b'\\') {
            return at + 2;
        }
    }

    s.len()
}

#[cfg(test)]
#[path = "region_tests.rs"]
mod tests;
