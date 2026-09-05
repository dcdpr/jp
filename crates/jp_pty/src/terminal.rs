//! The terminal a test drives, and the plumbing that keeps its screen current.

use std::{
    borrow::Cow,
    io::{self, Read as _, Write},
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    thread,
    time::{Duration, Instant},
};

// `portable_pty` names a pty's two ends with the older master/slave pair. The
// aliases are POSIX Issue 8's wording for the same two things, so one
// vocabulary runs through this crate.
use portable_pty::{
    Child as PtyChild, CommandBuilder, MasterPty as ManagerPty, PtyPair, PtySize,
    SlavePty as SubsidiaryPty, native_pty_system,
};

use crate::{Error, screen::Screen};

/// Rows the screen model keeps above the viewport.
///
/// Enough that a case which scrolls rows off the top can still tell "pushed
/// into history" apart from "destroyed".
const SCROLLBACK: usize = 100;

/// How long [`Terminal::wait_for`] waits before giving up.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Bytes read from the pty in one go.
const READ_CHUNK: usize = 4096;

/// A terminal's row and column count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    /// How many rows the viewport has.
    pub rows: u16,

    /// How many columns the viewport has.
    pub columns: u16,
}

impl Size {
    /// A viewport `rows` tall and `columns` wide.
    #[must_use]
    pub const fn new(rows: u16, columns: u16) -> Self {
        Self { rows, columns }
    }
}

impl From<Size> for PtySize {
    fn from(size: Size) -> Self {
        Self {
            rows: size.rows,
            cols: size.columns,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

/// What is between the code under test and the screen model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// A real pty: the writer is a tty, output crosses the kernel's line
    /// discipline, and a child process can be spawned into it.
    Pty,

    /// The screen model alone, written to directly.
    Modelled,
}

/// A terminal to draw into and then ask what it is showing.
///
/// See the crate documentation for how the two backends differ and when to
/// reach for which constructor.
pub struct Terminal {
    /// The screen model, shared with whatever feeds it.
    shared: Arc<Shared>,

    /// The pty, when this terminal has one.
    pty: Option<Pty>,

    /// How long [`Self::wait_for`] waits before giving up.
    timeout: Duration,
}

impl Terminal {
    /// A terminal of `size`, on the most faithful backend the platform offers.
    ///
    /// A pty where the parent can write into one, the screen model alone
    /// everywhere else — which is Windows, whose ConPTY is only reachable by a
    /// child process, and anywhere `openpty` fails.
    /// [`Self::writer`] is guaranteed to succeed either way; [`Self::spawn`]
    /// and [`Self::send`] need [`Backend::Pty`], which [`Self::backend`]
    /// reports.
    #[must_use]
    pub fn open(size: Size) -> Self {
        match Self::pty(size) {
            Ok(terminal) if terminal.writer().is_ok() => terminal,
            _ => Self::modelled(size),
        }
    }

    /// A terminal of `size` backed by a real pty.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoPty`] where the platform has no pty, or where the
    /// process has exhausted the ones it may open.
    pub fn pty(size: Size) -> Result<Self, Error> {
        let PtyPair {
            slave: subsidiary,
            master: manager,
        } = native_pty_system()
            .openpty(size.into())
            .map_err(|error| Error::NoPty(error.to_string()))?;

        let reader = manager
            .try_clone_reader()
            .map_err(|error| Error::NoPty(error.to_string()))?;
        let input = manager
            .take_writer()
            .map_err(|error| Error::NoPty(error.to_string()))?;

        let shared = Arc::new(Shared::new(size, true));
        spawn_reader(reader, Arc::clone(&shared));

        Ok(Self {
            shared,
            pty: Some(Pty {
                subsidiary,
                manager,
                input: Mutex::new(input),
            }),
            timeout: DEFAULT_TIMEOUT,
        })
    }

    /// A terminal of `size` backed by the screen model alone.
    ///
    /// Writes land synchronously, so a screen read straight after one sees it.
    /// The writer translates `\n` to `\r\n` the way a tty's `ONLCR` does, so a
    /// test reads the same on either backend.
    #[must_use]
    pub fn modelled(size: Size) -> Self {
        Self {
            shared: Arc::new(Shared::new(size, false)),
            pty: None,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Wait at most `timeout` in [`Self::wait_for`], rather than five seconds.
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// What is between the code under test and the screen model.
    #[must_use]
    pub const fn backend(&self) -> Backend {
        if self.pty.is_some() {
            Backend::Pty
        } else {
            Backend::Modelled
        }
    }

    /// A handle for the code under test to draw into.
    ///
    /// On a pty-backed terminal this is the subsidiary end, so the code sees a
    /// tty and its output crosses the line discipline on the way to the screen.
    /// Several writers can be open at once; they share one screen.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoLocalTty`] on a pty this platform only lets a child
    /// process write into.
    /// [`Self::open`] never yields such a terminal.
    pub fn writer(&self) -> Result<Writer, Error> {
        let sink = match &self.pty {
            Some(pty) => Sink::Tty(open_tty(pty.manager.as_ref())?),
            None => Sink::Model(Arc::clone(&self.shared)),
        };

        Ok(Writer { sink })
    }

    /// What the terminal is showing right now.
    ///
    /// On a pty-backed terminal bytes arrive on a reader thread, so this can
    /// catch a half-drawn frame; [`Self::wait_for`] is what makes an assertion
    /// deterministic.
    #[must_use]
    pub fn screen(&self) -> Screen {
        Screen::capture(self.shared.lock().parser.screen())
    }

    /// Block until the screen satisfies `predicate`, and return the screen that
    /// did.
    ///
    /// `what` names the thing being waited for, and is what a timeout reports
    /// alongside the screen as it stood.
    /// The wait is driven by arriving bytes, not by the clock.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Timeout`] when the predicate has not held by the
    /// terminal's timeout, and [`Error::Stalled`] when nothing can change the
    /// screen any more — the pty has closed, or the terminal is model-backed
    /// and only a write can move it.
    pub fn wait_for(
        &self,
        what: &str,
        predicate: impl Fn(&Screen) -> bool,
    ) -> Result<Screen, Error> {
        let started = Instant::now();
        let mut state = self.shared.lock();

        loop {
            let screen = Screen::capture(state.parser.screen());
            if predicate(&screen) {
                return Ok(screen);
            }

            if !state.live {
                return Err(Error::Stalled {
                    what: what.to_owned(),
                    screen,
                });
            }

            let remaining = self.timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err(Error::Timeout {
                    what: what.to_owned(),
                    elapsed: started.elapsed(),
                    screen,
                });
            }

            let (guard, _timed_out) = self
                .shared
                .updated
                .wait_timeout(state, remaining)
                .unwrap_or_else(PoisonError::into_inner);
            state = guard;
        }
    }

    /// Resize the terminal, as dragging the window would.
    ///
    /// A pty-backed terminal tells the kernel, which signals any child; the
    /// screen model is resized either way.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Resize`] when the kernel refuses the new size.
    pub fn resize(&self, size: Size) -> Result<(), Error> {
        if let Some(pty) = &self.pty {
            pty.manager
                .resize(size.into())
                .map_err(|error| Error::Resize(error.to_string()))?;
        }

        let mut state = self.shared.lock();
        state.parser.screen_mut().set_size(size.rows, size.columns);
        drop(state);
        self.shared.updated.notify_all();

        Ok(())
    }

    /// Type `keys` into the terminal, as a user at the keyboard would.
    ///
    /// The bytes go in as input, so whether they appear on screen is the
    /// reading program's business: a program in canonical mode has them echoed
    /// by the line discipline, one in raw mode does not.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotAPty`] on a model-backed terminal, which has nobody
    /// to type at.
    pub fn send(&self, keys: &str) -> Result<(), Error> {
        let pty = self
            .pty
            .as_ref()
            .ok_or(Error::NotAPty("keystroke injection"))?;

        let mut input = pty.input.lock().unwrap_or_else(PoisonError::into_inner);
        input.write_all(keys.as_bytes())?;
        input.flush()?;
        drop(input);

        Ok(())
    }

    /// Run `command` in the terminal, with the pty as its stdin, stdout, and
    /// stderr.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotAPty`] on a model-backed terminal, and
    /// [`Error::Spawn`] when the command cannot be started.
    pub fn spawn(&self, command: CommandBuilder) -> Result<Child, Error> {
        let pty = self
            .pty
            .as_ref()
            .ok_or(Error::NotAPty("spawning a child"))?;
        let child = pty
            .subsidiary
            .spawn_command(command)
            .map_err(|error| Error::Spawn(error.to_string()))?;

        Ok(Child { child })
    }
}

/// The pty behind a [`Terminal`], and the ends the harness holds open.
///
/// A pty has two ends: the *subsidiary*, which a program uses as its terminal,
/// and the *manager*, which stands where the terminal emulator would — it
/// reads what the program drew, delivers what the user typed, and owns the
/// size.
struct Pty {
    /// The end a child process is spawned into.
    ///
    /// Held open for the terminal's whole life, so the reader thread stays
    /// parked rather than seeing the pty close between writers.
    /// Listed first so it is dropped first, per `portable_pty`'s own ordering.
    subsidiary: Box<dyn SubsidiaryPty + Send>,

    /// The end that reads what was drawn and sets the size.
    manager: Box<dyn ManagerPty + Send>,

    /// The manager's write end, taken once because taking it twice is invalid.
    input: Mutex<Box<dyn Write + Send>>,
}

/// A child process running in a terminal.
///
/// Killed on drop, so a test that stops caring about a child does not leave one
/// behind.
pub struct Child {
    /// The spawned process.
    child: Box<dyn PtyChild + Send + Sync>,
}

impl Child {
    /// Whether the child has exited, without blocking.
    ///
    /// # Errors
    ///
    /// Returns the error the platform reported for the check itself.
    pub fn finished(&mut self) -> Result<bool, Error> {
        Ok(self.child.try_wait()?.is_some())
    }

    /// Block until the child exits, and report whether it exited successfully.
    ///
    /// # Errors
    ///
    /// Returns the error the platform reported for the wait itself.
    pub fn wait(&mut self) -> Result<bool, Error> {
        Ok(self.child.wait()?.success())
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        // A child that has already exited cannot be killed, and that is the
        // ordinary case rather than a failure.
        let _err = self.child.kill();
    }
}

/// A handle the code under test draws into.
pub struct Writer {
    /// Where the bytes go.
    sink: Sink,
}

/// The two places a [`Writer`]'s bytes can go.
enum Sink {
    /// A pty's subsidiary end.
    Tty(std::fs::File),

    /// The screen model, written to directly.
    Model(Arc<Shared>),
}

impl Write for Writer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match &mut self.sink {
            Sink::Tty(file) => file.write(buf),
            Sink::Model(shared) => {
                shared.feed(&onlcr(buf));
                Ok(buf.len())
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.sink {
            Sink::Tty(file) => file.flush(),
            Sink::Model(_) => Ok(()),
        }
    }
}

/// The screen model and the parties waiting on it.
struct Shared {
    /// The model, and whether anything can still reach it unprompted.
    state: Mutex<State>,

    /// Signalled whenever the screen changes or stops being able to.
    updated: Condvar,
}

impl Shared {
    /// A blank screen of `size`, `live` if bytes can arrive on their own.
    fn new(size: Size, live: bool) -> Self {
        Self {
            state: Mutex::new(State {
                parser: vt100::Parser::new(size.rows, size.columns, SCROLLBACK),
                live,
            }),
            updated: Condvar::new(),
        }
    }

    /// Take the state, ignoring a poisoned lock.
    ///
    /// A panicking test has already failed; refusing to report the screen
    /// afterwards only hides why.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Apply `bytes` to the screen and wake anything waiting on it.
    fn feed(&self, bytes: &[u8]) {
        let mut state = self.lock();
        state.parser.process(bytes);
        drop(state);
        self.updated.notify_all();
    }

    /// Record that nothing further will reach the screen.
    fn close(&self) {
        let mut state = self.lock();
        state.live = false;
        drop(state);
        self.updated.notify_all();
    }
}

/// The screen model, and whether it can still move on its own.
struct State {
    /// What a terminal would be showing.
    parser: vt100::Parser,

    /// Whether bytes can still arrive without a write.
    ///
    /// True while a pty's reader thread runs, false for a model-backed
    /// terminal, which only moves when written to.
    live: bool,
}

/// Feed everything the pty emits into `shared`, until it closes.
///
/// The thread is never joined: it unparks only when the last handle on the
/// subsidiary end closes, and a spawned child may hold one past the end of a
/// test.
/// It costs a stack until the process exits.
fn spawn_reader(mut reader: Box<dyn io::Read + Send>, shared: Arc<Shared>) {
    thread::spawn(move || {
        let mut buffer = [0_u8; READ_CHUNK];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => shared.feed(&buffer[..count]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => (),
                // Reading the manager end reports the last subsidiary handle
                // closing as an error on Linux and as end-of-file on macOS.
                // Both mean the same thing.
                Err(_) => break,
            }
        }

        shared.close();
    });
}

/// Translate `\n` to `\r\n`, as a tty's `ONLCR` does on the way out.
///
/// Unconditional, like the terminal driver: an existing `\r\n` becomes
/// `\r\r\n`, which renders the same.
fn onlcr(bytes: &[u8]) -> Cow<'_, [u8]> {
    if !bytes.contains(&b'\n') {
        return Cow::Borrowed(bytes);
    }

    let mut out = Vec::with_capacity(bytes.len() + 8);
    for &byte in bytes {
        if byte == b'\n' {
            out.push(b'\r');
        }
        out.push(byte);
    }

    Cow::Owned(out)
}

/// Open a writable handle on the pty's subsidiary end.
#[cfg(unix)]
fn open_tty(manager: &dyn ManagerPty) -> Result<std::fs::File, Error> {
    use std::{fs::OpenOptions, os::unix::fs::OpenOptionsExt as _};

    let path = manager.tty_name().ok_or(Error::NoLocalTty)?;

    // `O_NOCTTY`: writing into the pty must never make it this process's
    // controlling terminal, which would redirect its own signals.
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open(path)
        .map_err(Error::from)
}

/// Open a writable handle on the pty's subsidiary end.
#[cfg(not(unix))]
fn open_tty(_manager: &dyn ManagerPty) -> Result<std::fs::File, Error> {
    Err(Error::NoLocalTty)
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;
