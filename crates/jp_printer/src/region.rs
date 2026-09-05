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
    collections::{HashSet, VecDeque},
    fmt::{self, Write as _},
    io::{self, IsTerminal as _},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::Sender,
    },
    time::{Duration, Instant},
};

use jp_term::width::{display_width, prefix_end_for_width};
use parking_lot::Mutex;

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

/// CSI cursor-up one row.
const CURSOR_UP: &str = "\x1b[1A";

/// SGR background reset, closing a row background so it never leaks below the
/// region.
const BACKGROUND_END: &str = "\x1b[49m";

/// SGR foreground reset, closing a source label's colour.
///
/// Deliberately not a full reset: a region drawn inside a reasoning block has a
/// row background active, and `\x1b[0m` would clear it mid-row.
const FOREGROUND_END: &str = "\x1b[39m";

/// SGR foreground colours a source label can be drawn in.
///
/// The basic palette rather than the 256-colour cube, so the labels follow the
/// user's terminal theme instead of pinning fixed RGB.
/// Red is left out because it reads as failure, and so are black, white, and
/// the greys, which disappear against one background or another.
const LABEL_COLOURS: [u8; 10] = [36, 35, 32, 33, 34, 96, 95, 92, 93, 94];

/// SGR reset, appended to a filtered line that left an attribute open so child
/// styling cannot bleed into the region's own rows.
const SGR_RESET: &str = "\x1b[0m";

/// Lines a region's window buffer holds before evicting its oldest.
///
/// An upper bound rather than the window's height: the height is re-resolved on
/// every draw, and a buffer sized for the terminal as it was at claim time
/// would come up short after the window grew.
const WINDOW_CAPACITY: usize = 256;

/// Rows of terminal that must stay free of the region: the status row itself,
/// plus one row of context above it.
///
/// A window that fills the screen leaves nowhere for the erase to walk back to,
/// and a terminal shrunk below the drawn row count cannot be cleaned up at all
/// — the rows above the viewport are already in scrollback.
const RESERVED_ROWS: u16 = 2;

/// Divisor turning the terminal's height into the automatic window size.
const AUTO_WINDOW_DIVISOR: u16 = 10;

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

    /// The terminal's row count, when it could be measured.
    rows: Option<u16>,

    /// Whether the size came from a real terminal, and so has to be re-measured
    /// rather than trusted after the window is resized.
    measured: bool,

    /// Whether a tracing layer writes to stderr.
    stderr_logging: bool,
}

impl TerminalCapability {
    /// Measure the chrome channel: stderr's tty-ness and the terminal's size.
    ///
    /// Yields a non-interactive capability when stderr is piped or redirected.
    #[must_use]
    pub fn detect() -> Self {
        if !io::stderr().is_terminal() {
            return Self::default();
        }

        let size = crossterm::terminal::size().ok();

        Self {
            interactive: true,
            columns: size.map(|(columns, _)| columns),
            rows: size.map(|(_, rows)| rows),
            measured: true,
            stderr_logging: false,
        }
    }

    /// An interactive terminal `columns` wide, declared rather than measured.
    ///
    /// Pass `None` for `columns` to model a terminal whose width could not be
    /// determined; rows are then left unbounded.
    /// A declared terminal has no height until [`Self::with_rows`] gives it
    /// one, so it renders a bare status row and no window.
    #[must_use]
    pub const fn interactive(columns: Option<u16>) -> Self {
        Self {
            interactive: true,
            columns,
            rows: None,
            measured: false,
            stderr_logging: false,
        }
    }

    /// Declare the terminal's height, which is what sizes the output window.
    #[must_use]
    pub const fn with_rows(mut self, rows: Option<u16>) -> Self {
        self.rows = rows;
        self
    }

    /// The terminal's size right now.
    ///
    /// A measured terminal is re-measured on every call rather than trusted: a
    /// window that shrank below the drawn row count turns a captured height
    /// into a hazard, because the erase then walks up further than the viewport
    /// has rows and clears content that was never the region's.
    fn live_size(self) -> (Option<u16>, Option<u16>) {
        if !self.measured {
            return (self.columns, self.rows);
        }

        crossterm::terminal::size()
            .ok()
            .map_or((self.columns, self.rows), |(columns, rows)| {
                (Some(columns), Some(rows))
            })
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

/// How many rows of source output a region shows above its status row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputLines {
    /// No window: the region is the status row alone.
    #[default]
    Off,

    /// Sized from the terminal's height, a tenth of it.
    Auto,

    /// Exactly this many rows, still bounded by the terminal's height.
    Rows(u16),
}

impl OutputLines {
    /// Window rows to show on a terminal `height` rows tall.
    ///
    /// Zero when the height is unknown: the worker cannot bound a window it
    /// cannot size, and an unbounded one is what makes an erase eat content.
    fn rows(self, height: Option<u16>) -> usize {
        let Some(height) = height else {
            return 0;
        };

        let budget = height.saturating_sub(RESERVED_ROWS);
        let wanted = match self {
            Self::Off => 0,
            Self::Auto => height / AUTO_WINDOW_DIVISOR,
            Self::Rows(rows) => rows,
        };

        usize::from(wanted.min(budget))
    }
}

/// How a status region renders and ticks.
pub struct RegionStyle {
    /// How long after the claim the region stays invisible.
    delay: Duration,

    /// How often the status row redraws once it is visible.
    interval: Duration,

    /// How many rows of source output sit above the status row.
    output: OutputLines,

    /// The detail the first frame renders with, before any update arrives.
    detail: Option<String>,

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
            output: OutputLines::Off,
            detail: None,
            format: Box::new(format),
        }
    }

    /// Show up to `output` rows of source output above the status row.
    ///
    /// Defaults to [`OutputLines::Off`], which is the status row alone.
    #[must_use]
    pub const fn with_output(mut self, output: OutputLines) -> Self {
        self.output = output;
        self
    }

    /// The detail the first frame is rendered with.
    ///
    /// A zero-delay region paints the moment it is claimed, which is before any
    /// [`StatusRegion::set_detail`] can reach the worker.
    /// A client whose row reads wrong without a detail supplies it here rather
    /// than letting one frame of the fallback through.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

impl fmt::Debug for RegionStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegionStyle")
            .field("delay", &self.delay)
            .field("interval", &self.interval)
            .field("output", &self.output)
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

    /// The window this claim's sources push into, shared with the worker.
    buffer: Option<Arc<Mutex<WindowBuffer>>>,

    /// Whether a refresh is already on its way to the worker.
    refresh: Arc<AtomicBool>,
}

impl StatusRegion {
    /// A handle over a live claim, sharing `buffer` and `refresh` with it.
    pub(crate) const fn new(
        id: RegionId,
        tx: Sender<Command>,
        buffer: Arc<Mutex<WindowBuffer>>,
        refresh: Arc<AtomicBool>,
    ) -> Self {
        Self {
            region: Some(RegionRef { id, tx }),
            buffer: Some(buffer),
            refresh,
        }
    }

    /// A handle that renders nothing.
    ///
    /// For clients with their own enable switch — a `style.*.show` config key,
    /// say — so a disabled indicator is the same inert handle a disabled
    /// terminal produces, rather than an `Option` every call site unwraps.
    #[must_use]
    pub fn inert() -> Self {
        Self {
            region: None,
            buffer: None,
            refresh: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Whether this handle is backed by a live claim.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.region.is_some()
    }

    /// Release the claim, erasing the region's rows.
    ///
    /// The handle goes inert: later calls on it do nothing, and dropping it
    /// releases nothing further.
    /// Equivalent to dropping the handle, for a caller that holds it in a
    /// binding it cannot drop yet.
    pub fn release(&mut self) {
        if let Some(region) = self.region.take() {
            region.send(RegionCommand::Release { id: region.id });
        }
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

    /// Attach a named source and get a sink for pushing its lines.
    ///
    /// Several sources can feed one region; they share a single rolling window
    /// rather than getting one each, and every line is labelled as soon as the
    /// window holds more than one source's output.
    ///
    /// The source is open for as long as any clone of the sink lives.
    /// Dropping them all simply stops the pushes: lines already in the window
    /// stay there, labelled, until they are evicted.
    #[must_use]
    pub fn source(&self, label: impl Into<String>) -> LineSink {
        LineSink {
            buffer: self.buffer.clone(),
            refresh: Arc::clone(&self.refresh),
            region: self.region.clone(),
            label: Arc::from(label.into()),
        }
    }
}

impl Drop for StatusRegion {
    fn drop(&mut self) {
        self.release();
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

/// The rolling window a region's sources push into, shared with the worker.
///
/// Lines land here directly rather than travelling as commands.
/// The printer's command channel is unbounded, so a queued line is memory that
/// grows for as long as the worker is parked — inside a typewriter sleep, or
/// suspended for a prompt or an external editor — and a child that floods
/// during one of those pauses would queue every line it wrote.
/// A shared buffer bounds that to its capacity whatever the worker is doing.
#[derive(Debug, Default)]
pub struct WindowBuffer {
    /// The most recent lines, oldest first.
    lines: VecDeque<WindowLine>,
}

impl WindowBuffer {
    /// Add a line, evicting the oldest once the buffer is full.
    fn push(&mut self, label: Arc<str>, line: &str) {
        self.lines.push_back(WindowLine {
            label,
            text: filter_line(line),
        });

        while self.lines.len() > WINDOW_CAPACITY {
            self.lines.pop_front();
        }
    }
}

/// Pushes one source's output into a region's rolling window.
///
/// Cloneable, so a producer can hand copies around; the source stays open until
/// the last clone drops.
#[derive(Debug, Clone)]
pub struct LineSink {
    /// The window shared with the worker; `None` when the region is inert.
    buffer: Option<Arc<Mutex<WindowBuffer>>>,

    /// Whether a refresh is already on its way to the worker.
    refresh: Arc<AtomicBool>,

    /// The channel a refresh is raised on, and the claim it refers to.
    region: Option<RegionRef>,

    /// The name this source's lines are labelled with.
    label: Arc<str>,
}

impl LineSink {
    /// Add a line to the region's window.
    ///
    /// The line is filtered down to styling, bounded to the terminal's width
    /// when it is drawn, and evicted once newer lines push it out.
    /// Pushing never blocks and never waits on the display: a source that
    /// out-runs the terminal loses its oldest lines, which are the ones the
    /// window would have dropped anyway.
    ///
    /// A burst raises at most one redraw between the worker's wakeups, so a
    /// thousand lines cost one command rather than a thousand sitting in front
    /// of the next persistent write.
    pub fn push(&self, line: impl Into<String>) {
        let (Some(buffer), Some(region)) = (&self.buffer, &self.region) else {
            return;
        };

        buffer.lock().push(Arc::clone(&self.label), &line.into());

        if !self.refresh.swap(true, Ordering::Release) {
            region.send(RegionCommand::Refresh { id: region.id });
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

        /// The chrome channel's terminal, which sizes and bounds the rows.
        terminal: TerminalCapability,

        /// The window this claim's sources push into.
        buffer: Arc<Mutex<WindowBuffer>>,

        /// Cleared by the worker when it repaints, so the next push raises a
        /// fresh command.
        refresh: Arc<AtomicBool>,
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

    /// Repaint a claim whose sources have pushed since the last draw.
    ///
    /// Carries no lines: they are already in the claim's shared buffer.
    /// This only tells the worker there is something new to show.
    Refresh {
        /// The claim to repaint.
        id: RegionId,
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

/// One line held in a region's rolling window.
#[derive(Debug)]
struct WindowLine {
    /// The source that pushed it.
    label: Arc<str>,

    /// The line, already filtered down to styling.
    text: String,
}

/// One claimed region as the worker sees it.
struct RegionEntry {
    /// The claim's identifier.
    id: RegionId,

    /// When the claim was processed, and therefore what the row's elapsed time
    /// counts from.
    claimed_at: Instant,

    /// How the rows render and tick.
    style: RegionStyle,

    /// The chrome channel's terminal, re-measured whenever the rows are built.
    terminal: TerminalCapability,

    /// The detail passed to the format closure.
    detail: Option<String>,

    /// The pre-rendered SGR background escape for the rows, when one is set.
    background: Option<String>,

    /// The window this claim's sources push into, shared with them.
    buffer: Arc<Mutex<WindowBuffer>>,

    /// Cleared when the rows are repainted, so the next push raises a command.
    refresh: Arc<AtomicBool>,
}

impl RegionEntry {
    /// Whether the claim's delay has passed and the rows belong on screen.
    fn is_visible(&self) -> bool {
        self.claimed_at.elapsed() >= self.style.delay
    }

    /// The physical rows to paint: window rows above, status row last.
    ///
    /// Every row is bounded to the terminal's current width, and the whole
    /// block to its current height, so a resize can never leave the worker
    /// walking up more rows than the viewport has.
    fn rows(&self) -> Vec<String> {
        let (columns, height) = self.terminal.live_size();

        let secs = self.claimed_at.elapsed().as_secs_f64();
        let status = (self.style.format)(secs, self.detail.as_deref());

        // The buffer holds more than the window shows, so what is on screen is
        // its tail. Labelling and alignment key on that tail, not the buffer:
        // a source whose lines have all scrolled out is not in the window and
        // must not widen the labels of the ones that are.
        let buffer = self.buffer.lock();
        let window = self.style.output.rows(height).min(buffer.lines.len());
        let visible = || buffer.lines.iter().skip(buffer.lines.len() - window);

        let labelled = visible()
            .map(|line| line.label.as_ref())
            .collect::<HashSet<_>>()
            .len()
            > 1;
        let pad = visible()
            .map(|line| line.label.chars().count())
            .max()
            .unwrap_or(0);

        let mut rows: Vec<String> = visible()
            .map(|line| {
                if labelled {
                    let colour = label_colour(&line.label);
                    format!(
                        "\x1b[{colour}m[{:<pad$}]{FOREGROUND_END} {}",
                        line.label, line.text
                    )
                } else {
                    line.text.clone()
                }
            })
            .collect();
        drop(buffer);
        rows.push(status);

        if let Some(columns) = columns {
            for row in &mut rows {
                *row = truncate_row(row, usize::from(columns));
            }
        }

        rows
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
            RegionCommand::Claim {
                id,
                style,
                terminal,
                buffer,
                refresh,
            } => {
                self.claim(id, style, terminal, buffer, refresh, writer);
            }
            RegionCommand::Refresh { id } => self.refresh(id, writer),
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
        mut style: RegionStyle,
        terminal: TerminalCapability,
        buffer: Arc<Mutex<WindowBuffer>>,
        refresh: Arc<AtomicBool>,
        writer: &mut dyn io::Write,
    ) {
        self.erase(writer);
        self.entries.push(RegionEntry {
            id,
            claimed_at: Instant::now(),
            detail: style.detail.take(),
            style,
            terminal,
            background: None,
            buffer,
            refresh,
        });
        self.redraw(writer);
    }

    /// Fill a claim's window the way a [`LineSink`] does, without repainting.
    ///
    /// The production path writes into the shared buffer and raises a coalesced
    /// refresh separately; keeping the two apart lets a test stage a window and
    /// then assert on exactly one frame.
    #[cfg(test)]
    fn push(&self, id: RegionId, label: Arc<str>, line: &str) {
        if let Some(entry) = self.entries.iter().find(|entry| entry.id == id) {
            entry.buffer.lock().push(label, line);
        }
    }

    /// Claim with a buffer and refresh flag of its own, for tests that never
    /// hand a sink out.
    #[cfg(test)]
    fn claim_test(
        &mut self,
        id: RegionId,
        style: RegionStyle,
        terminal: TerminalCapability,
        writer: &mut dyn io::Write,
    ) {
        self.claim(
            id,
            style,
            terminal,
            Arc::new(Mutex::new(WindowBuffer::default())),
            Arc::new(AtomicBool::new(false)),
            writer,
        );
    }

    /// Repaint a claim whose sources have pushed since the last draw.
    ///
    /// The lines are already in the claim's buffer; clearing the flag before
    /// painting means a push that lands during the paint raises a fresh command
    /// rather than being swallowed.
    fn refresh(&mut self, id: RegionId, writer: &mut dyn io::Write) {
        let Some(entry) = self.entries.iter().find(|entry| entry.id == id) else {
            return;
        };

        entry.refresh.store(false, Ordering::Release);
        self.redraw_if_top(id, writer);
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

    /// Clear every row the worker painted, leaving the cursor where the region
    /// began.
    ///
    /// The walk is capped at the terminal's current height.
    /// A window shrunk below the drawn row count has already lost its top rows
    /// to scrollback and they cannot be reached again; walking up anyway would
    /// clear content that was never the region's, which is the worse of the two
    /// failures.
    pub fn erase(&mut self, writer: &mut dyn io::Write) {
        if self.drawn_rows == 0 {
            return;
        }

        let top = self.entries.last();
        let background = top.and_then(|entry| entry.background.clone());
        let reachable = top
            .and_then(|entry| entry.terminal.live_size().1)
            .map_or(self.drawn_rows, |height| {
                self.drawn_rows.min(usize::from(height))
            });

        let mut frame = String::new();
        for index in 0..reachable {
            if index > 0 {
                frame.push_str(CURSOR_UP);
            }
            push_erase(&mut frame, background.as_deref());
        }

        write_frame(writer, &frame);
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

        let rows = entry.rows();
        let background = entry.background.clone();
        let mut frame = String::new();

        // The cursor sits on the last row the region owns, and owns at least
        // that one even before its first paint. Rows the block has gained since
        // then are scrolled into existence first, so nothing is painted into a
        // row the terminal has not made yet; then the cursor walks back to the
        // block's first row. Only the rows *below* the cursor's own need
        // reserving, which is what keeps the block flush against the bottom of
        // the screen instead of one row short of it.
        let anchored = self.drawn_rows.max(1);
        let grown = rows.len().saturating_sub(anchored);
        for _ in 0..grown {
            frame.push('\n');
        }

        // No clearing on the way back up: every row the fill writes clears
        // itself.
        let back = anchored - 1 + grown;
        if back > 0 {
            let _err = write!(frame, "\x1b[{back}A");
        }

        for (index, row) in rows.iter().enumerate() {
            if index > 0 {
                frame.push('\n');
            }
            push_row(&mut frame, row, background.as_deref());
        }

        // A window that shrank leaves rows below the new block that nothing
        // will overwrite, so they are cleared explicitly and the cursor walks
        // back to the last painted row.
        let surplus = self.drawn_rows.saturating_sub(rows.len());
        for _ in 0..surplus {
            frame.push('\n');
            push_erase(&mut frame, background.as_deref());
        }
        if surplus > 0 {
            let _err = write!(frame, "\x1b[{surplus}A");
        }

        write_frame(writer, &frame);
        self.drawn_rows = rows.len();
    }

    /// Repaint only when `id` is the claim currently on screen.
    fn redraw_if_top(&mut self, id: RegionId, writer: &mut dyn io::Write) {
        if self.entries.last().is_some_and(|entry| entry.id == id) {
            self.redraw(writer);
        }
    }
}

/// Append a cleared row to `frame`, under `background` when one is set.
///
/// `\x1b[K` fills with whatever background is active, so the region re-asserts
/// its own before clearing and closes it afterwards; otherwise the erase
/// punches an unshaded hole in a reasoning block (RFD 095).
fn push_erase(frame: &mut String, background: Option<&str>) {
    frame.push('\r');
    if let Some(background) = background {
        frame.push_str(background);
    }
    frame.push_str(ERASE_LINE);
    if background.is_some() {
        frame.push_str(BACKGROUND_END);
    }
}

/// Append a painted row to `frame`, under `background` when one is set.
fn push_row(frame: &mut String, row: &str, background: Option<&str>) {
    frame.push('\r');
    if let Some(background) = background {
        frame.push_str(background);
    }
    frame.push_str(ERASE_LINE);
    frame.push_str(row);
    if background.is_some() {
        frame.push_str(BACKGROUND_END);
    }
}

/// Reduce a pushed line to styling, dropping everything that could move the
/// cursor or repaint the screen.
///
/// Region content comes from a child process JP did not write, so only SGR
/// survives: colours, bold, italic, underline.
/// Conceal (`SGR 8`) is dropped from the parameter list because text the reader
/// cannot see has no place in a preview.
/// Every other escape family — cursor movement, erasure, OSC, DCS — and every
/// bare control byte goes, since a child emitting `\x1b[2J` would wipe the
/// screen and one emitting cursor movement would corrupt the worker's own row
/// accounting.
///
/// A line that leaves an attribute open is closed with a reset, so child
/// styling cannot bleed into the rows below it.
fn filter_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut styled = false;
    let mut rest = line;

    while !rest.is_empty() {
        let text_end = rest.find('\x1b').unwrap_or(rest.len());
        if text_end > 0 {
            out.extend(rest[..text_end].chars().filter(|c| !c.is_control()));
            rest = &rest[text_end..];
            continue;
        }

        let end = escape_end(rest);
        if let Some(sgr) = visible_sgr(&rest[..end]) {
            out.push_str(&sgr);
            styled = true;
        }
        rest = &rest[end..];
    }

    if styled {
        out.push_str(SGR_RESET);
    }

    out
}

/// The SGR sequence `escape` carries with conceal removed, or `None` when it is
/// not an SGR sequence or has nothing left to say.
fn visible_sgr(escape: &str) -> Option<String> {
    let body = escape.strip_prefix("\x1b[")?.strip_suffix('m')?;

    // A bare `\x1b[m` is a reset, which is worth keeping as-is.
    if body.is_empty() {
        return Some(escape.to_owned());
    }

    let kept: Vec<&str> = body.split(';').filter(|param| *param != "8").collect();
    if kept.is_empty() {
        return None;
    }

    Some(format!("\x1b[{}m", kept.join(";")))
}

/// Write a region frame, discarding I/O errors.
///
/// A chrome row that cannot be written is not worth failing a run over, and the
/// worker has no channel to report it on.
fn write_frame(writer: &mut dyn io::Write, frame: &str) {
    let _err = writer.write_all(frame.as_bytes());
    let _err = writer.flush();
}

/// The SGR foreground colour a source's label is drawn in.
///
/// Derived from the name so a source keeps its colour across runs and across
/// machines.
/// Colours are only there to tell interleaved sources apart, so a collision
/// between two of them costs legibility, not correctness.
fn label_colour(label: &str) -> u8 {
    let index = label_hash(label) % LABEL_COLOURS.len() as u64;

    LABEL_COLOURS[usize::try_from(index).unwrap_or(0)]
}

/// FNV-1a over `label`'s bytes.
///
/// Hand-rolled rather than reached for from `std`: `DefaultHasher` makes no
/// promise across Rust releases and `RandomState` is seeded per process, and a
/// label's colour has to be the same one every time the user sees it.
fn label_hash(label: &str) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    label.bytes().fold(OFFSET, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(PRIME)
    })
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
