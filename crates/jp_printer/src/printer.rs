//! The printer module.

use std::{
    fmt::{self, Write},
    io,
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use parking_lot::Mutex;

use crate::typewriter::VisibleCharsIterator;

/// A shared buffer that can be written to.
pub type SharedBuffer = Arc<Mutex<String>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]

/// The format of the output.
pub enum Format {
    /// Plain text format.
    #[default]
    Text,
    /// JSON format.
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The mode of printing.
pub enum PrintMode {
    /// Print instantly.
    Instant,
    /// Print with a typewriter effect.
    Typewriter(Duration),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The target output stream.
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
    Flush(mpsc::Sender<()>),
    /// Shutdown the printer.
    Shutdown,
}
/// A centralized printer that handles output to out/err via a background
/// thread.
#[derive(Debug)]
pub struct Printer {
    /// The sender channel for print commands.
    tx: Sender<Command>,

    /// The output format.
    format: Format,

    /// The worker thread handle.
    worker_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl Clone for Printer {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            format: self.format,
            worker_handle: self.worker_handle.clone(),
        }
    }
}

impl Printer {
    /// Create a new printer with the given writers and format.
    pub fn new<O, E>(out: O, err: E, format: Format) -> Self
    where
        O: io::Write + Send + 'static,
        E: io::Write + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            let mut worker = Worker { out, err, rx };
            worker.run();
        });

        Self {
            tx,
            format,
            worker_handle: Arc::new(Mutex::new(Some(handle))),
        }
    }

    /// Create a new printer that writes to the terminal (stdout/stderr).
    #[must_use]
    pub fn terminal(format: Format) -> Self {
        Self::new(io::stdout(), io::stderr(), format)
    }

    /// Create a new printer that writes to memory buffers.
    #[must_use]
    pub fn memory(format: Format) -> (Self, SharedBuffer, SharedBuffer) {
        let out = Arc::new(Mutex::new(String::new()));
        let err = Arc::new(Mutex::new(String::new()));

        let out_w = SharedVec(out.clone());
        let err_w = SharedVec(err.clone());

        (Self::new(out_w, err_w, format), out, err)
    }
    /// Print content to stdout.
    pub fn print<P: Printable>(&self, p: P) {
        // We ignore send errors because they only happen during shutdown/panic
        drop(self.tx.send(Command::Print(p.into_task())));
    }

    /// Print content to stdout followed by a newline.
    pub fn println<P: Printable>(&self, p: P) {
        let mut task = p.into_task();
        task.content.push('\n');
        // We ignore send errors because they only happen during shutdown/panic
        drop(self.tx.send(Command::Print(task)));
    }

    /// Print content to stderr.
    pub fn eprint<P: Printable>(&self, p: P) {
        let mut task = p.into_task();
        task.target = PrintTarget::Err;
        // We ignore send errors because they only happen during shutdown/panic
        drop(self.tx.send(Command::Print(task)));
    }

    /// Print content to stderr followed by a newline.
    pub fn eprintln<P: Printable>(&self, p: P) {
        let mut task = p.into_task();
        task.content.push('\n');
        task.target = PrintTarget::Err;
        // We ignore send errors because they only happen during shutdown/panic
        drop(self.tx.send(Command::Print(task)));
    }

    /// Get the format of the printer.
    #[must_use]
    pub const fn format(&self) -> Format {
        self.format
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
    /// Useful for tests.
    pub fn flush(&self) {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(Command::Flush(tx)).is_ok() {
            let _ = rx.recv();
        }
    }

    /// Stop the background thread and wait for it to finish.
    ///
    /// # Panics
    ///
    /// Panics if the worker handle mutex is poisoned.
    pub fn shutdown(&self) {
        drop(self.tx.send(Command::Shutdown));
        let mut guard = self.worker_handle.lock();
        if let Some(handle) = guard.take() {
            drop(handle.join());
        }
    }
}

/// A writer wrapper for `Printer` that implements `std::fmt::Write`.
#[derive(Debug, Clone, Copy)]
pub struct PrinterWriter<'a> {
    /// The printer to write to.
    printer: &'a Printer,
    /// The target output stream.
    target: PrintTarget,
}

impl std::fmt::Write for PrinterWriter<'_> {
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
        let s = std::str::from_utf8(buf).map_err(io::Error::other)?;

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
                Command::Shutdown => break,
            }
        }
    }

    /// Process a single print task.
    fn process_task(&mut self, task: &PrintTask) {
        let writer: &mut dyn io::Write = match task.target {
            PrintTarget::Out => &mut self.out,
            PrintTarget::Err => &mut self.err,
        };

        match task.mode {
            PrintMode::Instant => {
                let _err = write!(writer, "{}", task.content);
                let _err = writer.flush();
            }
            PrintMode::Typewriter(delay) => {
                if delay.is_zero() {
                    let _err = write!(writer, "{}", task.content);
                    return;
                }

                for (c, visible) in VisibleCharsIterator::new(&task.content) {
                    let _err = write!(writer, "{c}");
                    let _err = writer.flush();

                    if visible {
                        std::thread::sleep(delay);
                    }
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_printer_async_ordering() {
        let (printer, out, _) = Printer::memory(Format::Text);

        printer.print("1");
        printer.print("234".typewriter(Duration::from_millis(10)));
        printer.print("5");

        // Wait for all tasks to complete
        printer.flush();

        assert_eq!(*out.lock(), "12345");
    }

    #[test]
    fn test_printer_targets() {
        let (printer, out, err) = Printer::memory(Format::Text);

        printer.println("Stdout");
        printer.eprintln("Stderr");

        printer.flush();

        assert_eq!(*out.lock(), "Stdout\n");
        assert_eq!(*err.lock(), "Stderr\n");
    }

    #[test]
    fn test_printer_writer() {
        let (printer, out, err) = Printer::memory(Format::Text);

        writeln!(printer.out_writer(), "Hello Writer").unwrap();
        writeln!(printer.err_writer(), "Error Writer").unwrap();

        printer.flush();

        assert_eq!(*out.lock(), "Hello Writer\n");
        assert_eq!(*err.lock(), "Error Writer\n");
    }
}
