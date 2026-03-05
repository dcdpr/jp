//! The printer module.

use std::{
    borrow::Cow,
    fmt::{self, Write},
    io,
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use parking_lot::{Condvar, Mutex};
use tracing::error;

use crate::typewriter::VisibleCharsIterator;

/// A shared buffer that can be written to.
pub type SharedBuffer = Arc<Mutex<String>>;

/// A centralized printer that handles output to out/err via a background
/// thread.
#[derive(Debug)]
pub struct Printer {
    /// The sender channel for print commands.
    tx: Sender<Command>,

    /// The output format.
    format: OutputFormat,

    /// The worker thread handle.
    worker_handle: Arc<Mutex<Option<JoinHandle<()>>>>,

    /// Shared with the background worker to interrupt typewriter sleeps.
    ///
    /// Set by `flush_instant()` before sending the `FlushInstant` command,
    /// so even a currently-running typewriter task wakes up immediately.
    /// Cleared by the worker after processing `FlushInstant`.
    delay_control: Arc<DelayControl>,
}

impl Clone for Printer {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            format: self.format,
            worker_handle: self.worker_handle.clone(),
            delay_control: self.delay_control.clone(),
        }
    }
}

impl Printer {
    /// Create a new printer with the given writers and format.
    pub fn new<O, E>(out: O, err: E, format: OutputFormat) -> Self
    where
        O: io::Write + Send + 'static,
        E: io::Write + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let delay_control = Arc::new(DelayControl {
            skip: Mutex::new(false),
            wake: Condvar::new(),
        });

        let handle = {
            let delay_control = delay_control.clone();
            thread::spawn(move || {
                let mut worker = Worker {
                    out,
                    err,
                    rx,
                    delay_control,
                    format,
                };
                worker.run();
            })
        };

        Self {
            tx,
            format,
            worker_handle: Arc::new(Mutex::new(Some(handle))),
            delay_control,
        }
    }

    /// Create a new printer that writes to the terminal (stdout/stderr).
    #[must_use]
    pub fn terminal(format: OutputFormat) -> Self {
        Self::new(io::stdout(), io::stderr(), format)
    }

    /// Create a new printer that silently discards all output.
    ///
    /// Useful when output should be suppressed entirely, e.g. when tool
    /// call rendering is disabled. The printer still spawns a background
    /// thread, but writes go to [`io::sink()`].
    #[must_use]
    pub fn sink() -> Self {
        Self::new(io::sink(), io::sink(), OutputFormat::Text)
    }

    /// Create a new printer that writes to memory buffers.
    #[must_use]
    pub fn memory(format: OutputFormat) -> (Self, SharedBuffer, SharedBuffer) {
        let out = Arc::new(Mutex::default());
        let err = Arc::new(Mutex::default());

        let out_w = SharedVec(out.clone());
        let err_w = SharedVec(err.clone());

        (Self::new(out_w, err_w, format), out, err)
    }

    /// Returns the output format.
    #[must_use]
    pub const fn format(&self) -> OutputFormat {
        self.format
    }

    /// Returns `true` if pretty printing is enabled (ANSI colors,
    /// unicode decorations, syntax highlighting).
    #[must_use]
    pub const fn pretty_printing(&self) -> bool {
        self.format.is_pretty()
    }
    /// Print content.
    ///
    /// In JSON mode, the content is wrapped in an NDJSON envelope:
    /// `{"message":"..."}\n`.
    pub fn print<P: Printable>(&self, p: P) {
        let mut task = p.into_task();
        if self.format.is_json() {
            task = self.wrap_json(task);
            task.content.push('\n');
        }

        self.send(Command::Print(task));
    }

    /// Print content followed by a newline.
    ///
    /// In JSON mode, the content is wrapped in an NDJSON envelope:
    /// `{"message":"..."}\n`.
    pub fn println<P: Printable>(&self, p: P) {
        let mut task = p.into_task();
        if self.format.is_json() {
            task = self.wrap_json(task);
        }

        task.content.push('\n');
        self.send(Command::Print(task));
    }

    /// Print pre-formatted content followed by a newline, without JSON
    /// wrapping.
    pub fn println_raw<P: Printable>(&self, p: P) {
        let mut task = p.into_task();
        task.content.push('\n');
        self.send(Command::Print(task));
    }

    /// Print error.
    pub fn eprint<P: Printable>(&self, p: P) {
        let mut task = p.into_task();
        task.target = PrintTarget::Err;
        self.send(Command::Print(task));
    }

    /// Print error followed by a newline.
    pub fn eprintln<P: Printable>(&self, p: P) {
        let mut task = p.into_task();
        task.content.push('\n');
        task.target = PrintTarget::Err;
        self.send(Command::Print(task));
    }

    /// Wrap a print task's content in an NDJSON envelope.
    ///
    /// ANSI escapes are stripped before serialization. This must happen here
    /// rather than in the worker because `serde_json` would encode ESC bytes as
    /// `\u001b`, making them invisible to the worker's `strip_ansi_escapes`
    /// pass.
    fn wrap_json(&self, mut task: PrintTask) -> PrintTask {
        let stripped = strip_ansi_escapes::strip_str(&task.content);
        let msg = stripped.trim_end_matches('\n');
        let json = serde_json::json!({ "message": msg });

        task.content = if self.format.is_json_pretty() {
            serde_json::to_string_pretty(&json).unwrap_or_else(|_| json.to_string())
        } else {
            serde_json::to_string(&json).unwrap_or_else(|_| json.to_string())
        };

        task
    }

    /// Get a writer that prints to the `out` stream.
    #[must_use]
    pub const fn out_writer(&self) -> PrinterWriter<'_> {
        PrinterWriter {
            printer: self,
            target: PrintTarget::Out,
        }
    }

    /// Get a writer that prints to the `err` stream.
    #[must_use]
    pub const fn err_writer(&self) -> PrinterWriter<'_> {
        PrinterWriter {
            printer: self,
            target: PrintTarget::Err,
        }
    }

    /// Block until all currently queued print tasks are finished.
    pub fn flush(&self) {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(Command::Flush(tx)).is_ok() {
            let _ = rx.recv();
        }
    }

    /// Flush all pending print tasks instantly, ignoring typewriter delays.
    ///
    /// Drains queued `Print` commands and writes their content immediately,
    /// then blocks until complete. Useful before showing interactive prompts
    /// (e.g., interrupt menus) to ensure all buffered output is visible
    /// without typewriter lag.
    pub fn flush_instant(&self) {
        // Set the flag and wake the worker so a currently-running
        // typewriter sleep returns immediately. The worker clears
        // the flag after draining.
        {
            *self.delay_control.skip.lock() = true;
        }
        self.delay_control.wake.notify_all();

        let (tx, rx) = mpsc::channel();
        if self.tx.send(Command::FlushInstant(tx)).is_ok() {
            let _ = rx.recv();
        }
    }

    /// Stop the background thread and wait for it to finish.
    pub fn shutdown(&self) {
        drop(self.tx.send(Command::Shutdown));
        let mut guard = self.worker_handle.lock();
        if let Some(handle) = guard.take() {
            drop(handle.join());
        }
    }

    /// Send a command to the background thread, printing an error if it fails.
    fn send(&self, command: Command) {
        if let Err(command) = self.tx.send(command) {
            error!(?command, "Failed to send command");
        }
    }
}

/// A writer wrapper for [`Printer`] that implements [`fmt::Write`].
#[derive(Debug, Clone, Copy)]
pub struct PrinterWriter<'a> {
    /// The printer to write to.
    printer: &'a Printer,

    /// The target output stream.
    target: PrintTarget,
}

impl fmt::Write for PrinterWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let task = PrintTask {
            content: s.to_owned(),
            mode: PrintMode::Instant,
            target: self.target,
        };

        self.printer
            .tx
            .send(Command::Print(task))
            .map_err(|_| fmt::Error)?;

        Ok(())
    }
}

impl io::Write for PrinterWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let s = str::from_utf8(buf).map_err(io::Error::other)?;

        self.write_str(s)
            .map_err(io::Error::other)
            .map(|()| s.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let (tx, rx) = mpsc::channel();
        self.printer
            .tx
            .send(Command::Flush(tx))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "printer shutdown"))?;

        rx.recv()
            .map_err(|_| io::Error::other("failed to receive flush signal"))
    }
}

/// The worker thread that processes print tasks.
struct Worker<O, E> {
    /// The `out` writer.
    out: O,

    /// The `err` writer.
    err: E,

    /// The receiver for print commands.
    rx: Receiver<Command>,

    /// Shared with the [`Printer`] to interrupt typewriter sleeps.
    delay_control: Arc<DelayControl>,

    /// The output format - controls ANSI stripping.
    format: OutputFormat,
}

impl<O: io::Write, E: io::Write> Worker<O, E> {
    /// Run the worker thread.
    fn run(&mut self) {
        while let Ok(cmd) = self.rx.recv() {
            match cmd {
                Command::Print(task) => self.process_task(&task),
                Command::Flush(tx) => {
                    // We don't need to do anything specific to flush out/err
                    // because we flush after every write in `process_task`. We
                    // just need to signal that we processed all messages prior
                    // to this one.
                    let _ = tx.send(());
                }
                Command::FlushInstant(tx) => {
                    self.drain_instant();
                    *self.delay_control.skip.lock() = false;
                    let _ = tx.send(());
                }
                Command::Shutdown => break,
            }
        }
    }

    /// Drain all pending commands, printing instantly.
    ///
    /// Processes queued `Print` tasks as `Instant` (ignoring typewriter
    /// delays). Honors `Flush` (signals the sender) and `Shutdown` (stops
    /// the worker) if encountered during the drain.
    fn drain_instant(&mut self) {
        while let Ok(cmd) = self.rx.try_recv() {
            match cmd {
                Command::Print(task) => self.process_task_instant(&task),
                Command::Flush(tx) => {
                    let _ = tx.send(());
                }
                Command::FlushInstant(tx) => {
                    // Nested FlushInstant: we're already draining, just signal.
                    let _ = tx.send(());
                }
                Command::Shutdown => break,
            }
        }
    }

    /// Process a print task instantly, ignoring typewriter delays.
    fn process_task_instant(&mut self, task: &PrintTask) {
        let content = self.maybe_strip(&task.content);

        let writer: &mut dyn io::Write = match task.target {
            PrintTarget::Out => &mut self.out,
            PrintTarget::Err => &mut self.err,
        };

        let _err = write!(writer, "{content}");
        let _err = writer.flush();
    }

    /// Process a single print task.
    fn process_task(&mut self, task: &PrintTask) {
        let PrintTask {
            content,
            mode,
            target,
        } = task;

        let content = self.maybe_strip(content);

        let writer: &mut dyn io::Write = match target {
            PrintTarget::Out => &mut self.out,
            PrintTarget::Err => &mut self.err,
        };

        match mode {
            PrintMode::Instant => {
                let _err = write!(writer, "{content}");
                let _err = writer.flush();
            }
            PrintMode::Typewriter(delay) => {
                if delay.is_zero() || *self.delay_control.skip.lock() {
                    let _err = write!(writer, "{content}");
                    let _err = writer.flush();
                    return;
                }

                for (c, visible) in VisibleCharsIterator::new(&content) {
                    let _err = write!(writer, "{c}");
                    let _err = writer.flush();

                    if visible {
                        let mut skip = self.delay_control.skip.lock();
                        if !*skip {
                            // Interruptible sleep: returns early if
                            // flush_instant() notifies the condvar.
                            self.delay_control.wake.wait_for(&mut skip, *delay);
                        }
                    }
                }
            }
        }
    }

    /// Strip ANSI escape sequences if the flag is set, otherwise return
    /// the input unchanged.
    fn maybe_strip<'a>(&self, content: &'a str) -> Cow<'a, str> {
        if self.format.is_pretty() {
            Cow::Borrowed(content)
        } else {
            Cow::Owned(strip_ansi_escapes::strip_str(content))
        }
    }
}

/// A shared buffer that can be written to.
struct SharedVec(SharedBuffer);

impl Write for SharedVec {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.lock().write_str(s)
    }
}

impl io::Write for SharedVec {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .write_str(String::from_utf8_lossy(buf).as_ref())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid UTF-8"))?;

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The output format controls how content is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// Plain text. No ANSI colors, no unicode decorations.
    #[default]
    Text,

    /// Pretty text. ANSI colors, unicode decorations, syntax highlighting.
    TextPretty,

    /// Compact JSON. Each print call becomes an NDJSON line.
    Json,

    /// Pretty-printed JSON. Indented, but no ANSI colors.
    JsonPretty,
}

impl OutputFormat {
    /// Whether this format uses ANSI colors and unicode decorations.
    #[must_use]
    pub const fn is_pretty(self) -> bool {
        matches!(self, Self::TextPretty)
    }

    /// Whether this format produces JSON output.
    #[must_use]
    pub const fn is_json(self) -> bool {
        matches!(self, Self::Json | Self::JsonPretty)
    }

    /// Whether JSON output should be indented.
    #[must_use]
    pub const fn is_json_pretty(self) -> bool {
        matches!(self, Self::JsonPretty)
    }
}

/// The mode of printing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintMode {
    /// Print instantly.
    Instant,

    /// Print with a typewriter effect.
    Typewriter(Duration),
}

/// The target output stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintTarget {
    /// Output stream.
    Out,

    /// Error stream.
    Err,
}

#[derive(Debug, Clone)]
/// A task to be printed.
pub struct PrintTask {
    /// The content to print.
    pub content: String,

    /// The mode of printing.
    pub mode: PrintMode,

    /// The target output stream.
    pub target: PrintTarget,
}

impl Default for PrintTask {
    fn default() -> Self {
        Self {
            content: String::new(),
            mode: PrintMode::Instant,
            target: PrintTarget::Out,
        }
    }
}

/// Trait for types that can be converted into a print task.
pub trait Printable {
    /// Convert into a print task.
    fn into_task(self) -> PrintTask;
}

impl Printable for String {
    fn into_task(self) -> PrintTask {
        PrintTask {
            content: self,
            ..Default::default()
        }
    }
}

impl Printable for &String {
    fn into_task(self) -> PrintTask {
        PrintTask {
            content: self.to_owned(),
            ..Default::default()
        }
    }
}

impl Printable for &str {
    fn into_task(self) -> PrintTask {
        PrintTask {
            content: self.to_owned(),
            ..Default::default()
        }
    }
}

impl Printable for PrintTask {
    fn into_task(self) -> PrintTask {
        self
    }
}

/// Wrapper struct to enable `.typewriter(duration)` syntax.
pub struct Typewriter<T>(pub T, pub Duration);

impl<T: Printable> Printable for Typewriter<T> {
    fn into_task(self) -> PrintTask {
        let mut task = self.0.into_task();
        task.mode = PrintMode::Typewriter(self.1);
        task
    }
}

/// Extension trait to add `.typewriter()` to strings and other printables.
pub trait PrintableExt: Sized + Printable {
    /// Print with a typewriter effect.
    fn typewriter(self, delay: Duration) -> Typewriter<Self> {
        Typewriter(self, delay)
    }

    /// Print to `err` instead of `out`.
    fn to_err(self) -> PrintTask {
        let mut task = self.into_task();
        task.target = PrintTarget::Err;
        task
    }
}

impl<T: Printable> PrintableExt for T {}

/// A command to be executed by the printer worker.
#[derive(Debug)]
enum Command {
    /// Print a task.
    Print(PrintTask),

    /// Flush the printer.
    ///
    /// The sender is used to signal the flush is complete.
    Flush(mpsc::Sender<()>),

    /// Flush all pending print tasks instantly, ignoring typewriter delays.
    ///
    /// Drains all queued `Print` commands and writes their content
    /// immediately (as `PrintMode::Instant`), then signals completion.
    /// Other commands (`Flush`, `Shutdown`) encountered during the drain
    /// are still honored.
    FlushInstant(mpsc::Sender<()>),

    /// Shutdown the printer.
    Shutdown,
}

/// Shared state for interruptible typewriter delays.
///
/// Bundles a flag with a [`Condvar`] so that [`Printer::flush_instant`]
/// can wake a sleeping worker immediately instead of waiting for the
/// current per-character delay to expire.
#[derive(Debug)]
struct DelayControl {
    /// When `true`, the worker skips all typewriter delays.
    skip: Mutex<bool>,

    /// Notified when `skip` transitions to `true`.
    wake: Condvar,
}

#[cfg(test)]
#[path = "printer_tests.rs"]
mod tests;
