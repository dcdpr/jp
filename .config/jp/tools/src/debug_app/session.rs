//! The running app the `debug_app_*` tools address.
//!
//! The app is a long-lived GUI process, so the running instance *is* the
//! session.
//! [`Session`] records which process `debug_app_launch` started and where that
//! process keeps its state; every other tool loads the record and calls
//! [`Session::resolve`] before touching anything.
//!
//! Resolution is what makes the implicit global state safe to address.
//! A tool that silently acted on whichever instance happened to be running
//! would report on an app the caller never started, so every mismatch is an
//! error naming what was expected, what was found, and what to do next.

use std::{
    fs, io,
    io::{Read as _, Seek as _, SeekFrom},
};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::{Context, Error};

/// How much of a console file to quote when reporting an app that is gone.
const TAIL_BYTES: u64 = 4096;

/// The app's own trace stream, inside the state directory.
const TRACE_FILE: &str = "trace.jsonl";

/// Where the app reports the ASLR slide of its own main image.
const SLIDE_FILE: &str = "slide";

/// Where the app writes its trace, given the directory it keeps state in.
pub(crate) fn trace_path(state_dir: &Utf8Path) -> Utf8PathBuf {
    state_dir.join(TRACE_FILE)
}

/// Where a slot's app keeps its state, whether or not a session is recorded.
///
/// Fixed by the slot rather than read from the session record, because reading
/// what a run left behind has to work after `debug_app_quit` has removed that
/// record.
pub(crate) fn state_dir(dir: &Utf8Path) -> Utf8PathBuf {
    dir.join("state")
}

/// A captured output stream and how much of it has been reported.
///
/// The default names no file and reads as empty forever.
/// It is what a session record written before a stream existed deserializes to,
/// and reading nothing is the right answer there: an app old enough to predate
/// the record also predates anything writing that stream.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Console {
    pub path: Utf8PathBuf,

    /// Byte offset up to which the stream has already been returned to a
    /// caller.
    pub offset: u64,
}

impl Console {
    pub(crate) fn new(path: Utf8PathBuf) -> Self {
        Self { path, offset: 0 }
    }

    /// Everything written since the last read, advancing the offset past it.
    ///
    /// A file shorter than the offset means it was truncated under us — a
    /// relaunch that reused the path, most likely — so the whole file is
    /// returned rather than nothing.
    pub(crate) fn delta(&mut self) -> Result<String, Error> {
        let mut file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(String::new()),
            Err(e) => return Err(format!("Failed to open {}: {e}", self.path).into()),
        };

        let size = file.metadata()?.len();
        if size < self.offset {
            self.offset = 0;
        }

        file.seek(SeekFrom::Start(self.offset))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;
        self.offset = size.max(self.offset + buf.len() as u64);

        Ok(String::from_utf8_lossy(&buf).into_owned())
    }

    /// The last [`TAIL_BYTES`] of the stream, regardless of the offset.
    ///
    /// For reporting on an app that is already gone, where what matters is what
    /// it said last rather than what has been reported before.
    pub(crate) fn tail(&self) -> String {
        let Ok(mut file) = fs::File::open(&self.path) else {
            return String::new();
        };
        let Ok(size) = file.metadata().map(|m| m.len()) else {
            return String::new();
        };

        if file
            .seek(SeekFrom::Start(size.saturating_sub(TAIL_BYTES)))
            .is_err()
        {
            return String::new();
        }

        let mut buf = Vec::new();
        if file.read_to_end(&mut buf).is_err() {
            return String::new();
        }

        String::from_utf8_lossy(&buf).into_owned()
    }
}

/// The app instance a `debug_app_*` tool acts on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Session {
    /// The process id the app reported at launch, read from `<state_dir>/pid`.
    pub pid: u32,

    /// The bundle that was launched.
    pub bundle: Utf8PathBuf,

    /// The Xcode configuration the bundle was built in.
    pub configuration: String,

    /// The workspace the app was pointed at.
    pub workspace: Utf8PathBuf,

    /// `JP_DEBUG_STATE_DIR`: where the app keeps its recents list and its pid.
    pub state_dir: Utf8PathBuf,

    /// `JP_USER_DATA_DIR`: the app's user-local conversation store.
    pub user_data_dir: Utf8PathBuf,

    pub stdout: Console,
    pub stderr: Console,

    /// The intervals and footprint samples the app writes about its own work.
    ///
    /// Kept off both console streams on purpose: those are reported as deltas
    /// on every snapshot, and a trace stream on either would bury whatever
    /// `AppKit` had to say under our own instrumentation.
    #[serde(default)]
    pub trace: Console,

    /// The memory footprint last reported to a caller, in MiB.
    ///
    /// Held across calls so a snapshot can say how far the footprint moved
    /// since the previous one rather than only within its own delta.
    #[serde(default)]
    pub reported_footprint_mb: Option<u64>,

    /// The dSYM matching the launched binary, when the build produced one.
    ///
    /// Recorded here because resolving it means asking `xcodebuild` where the
    /// build landed, and nothing that reads a profile afterwards has a reason
    /// to run a build.
    #[serde(default)]
    pub dsym: Option<Utf8PathBuf>,

    /// Whether the app keeps a stack for every allocation it makes.
    ///
    /// Set by launching under `MallocStackLogging`, which libmalloc reads at
    /// process start, so this is the whole answer and it cannot change while
    /// the app runs.
    /// A profile bracket asked for allocations against a session where this is
    /// false has to refuse and name the relaunch, rather than record an
    /// instrument that would find nothing.
    #[serde(default)]
    pub allocation_stacks: bool,
}

/// Environment variable overriding which slot a run is scoped to.
pub(crate) const SLOT_VAR: &str = "JP_DEBUG_APP_SLOT";

/// The slot a run with nothing to derive one from is scoped to.
const FALLBACK_SLOT: &str = "default";

/// One agent's private everything: its own session record, state and user-data
/// directories, console files, scratch workspace, and app bundle.
///
/// Derived from the conversation rather than defaulted, because a default is a
/// collision waiting for a second agent: nothing would make either of them
/// choose otherwise, and both would drive the same instance.
/// A conversation is already one agent driving one app, and an agent cannot
/// forget to be itself.
///
/// [`SLOT_VAR`] overrides it, for the case where two conversations should share
/// one running instance on purpose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Slot(String);

impl Slot {
    /// The slot this invocation is scoped to.
    pub(crate) fn for_context(ctx: &Context) -> Result<Slot, Error> {
        let named = std::env::var(SLOT_VAR).ok();
        Slot::named(named.as_deref(), &ctx.conversation_id)
    }

    /// The slot an override and a conversation resolve to.
    ///
    /// A slot ends up inside a reverse-DNS bundle identifier and in every path
    /// a run writes, so it is limited to letters, digits and hyphens.
    ///
    /// An override is held to that rather than filtered down to it.
    /// Filtering silently answers a different question than the one asked: `my
    /// slot` would become `myslot`, whose artifacts are not where the caller
    /// looks, and two deliberately distinct names could reduce to one and share
    /// an app.
    /// A conversation id is filtered instead, because nobody chose it and there
    /// is nobody to report it back to.
    fn named(overridden: Option<&str>, conversation: &str) -> Result<Slot, Error> {
        if let Some(given) = overridden.filter(|given| !given.is_empty()) {
            if !given.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Err(format!(
                    "`{SLOT_VAR}` is {given:?}, which cannot name a slot: a slot is used as a \
                     directory name and inside the app's bundle identifier, so it takes only \
                     letters, digits and hyphens."
                )
                .into());
            }

            return Ok(Slot(given.to_owned()));
        }

        let cleaned: String = conversation
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();

        if cleaned.is_empty() {
            return Ok(Slot(FALLBACK_SLOT.to_owned()));
        }

        Ok(Slot(cleaned))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
impl Slot {
    /// A slot with a fixed name, so a test's paths do not depend on the
    /// environment or on which conversation ran it.
    pub(crate) fn fixed(name: &str) -> Slot {
        Slot(name.to_owned())
    }
}

impl std::fmt::Display for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Session {
    /// The directory every artifact of a driven run lives under.
    pub(crate) fn dir(root: &Utf8Path, slot: &Slot) -> Utf8PathBuf {
        root.join("tmp/debug-app").join(slot.as_str())
    }

    /// Where the record itself is kept, inside a slot's directory.
    pub(crate) fn path(dir: &Utf8Path) -> Utf8PathBuf {
        dir.join("session.json")
    }

    /// Read the recorded session, or `None` when no run has been started.
    pub(crate) fn load(dir: &Utf8Path) -> Result<Option<Session>, Error> {
        let path = Self::path(dir);
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("Failed to read {path}: {e}").into()),
        };

        serde_json::from_str(&raw)
            .map(Some)
            .map_err(|e| format!("Failed to parse {path}: {e}. Remove it and launch again.").into())
    }

    /// Write the record, replacing any earlier one.
    pub(crate) fn store(&self, dir: &Utf8Path) -> Result<(), Error> {
        let path = Self::path(dir);
        fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(self)?;
        fs::write(&path, format!("{json}\n"))
            .map_err(|e| format!("Failed to write {path}: {e}").into())
    }

    /// Load the recorded session and verify the app it names is still the app
    /// that is running.
    ///
    /// Returns an error rather than a stale session, because acting on the
    /// wrong instance is worse than not acting: a snapshot of an app the caller
    /// never launched looks like a real answer.
    pub(crate) fn resolve(dir: &Utf8Path) -> Result<Session, Error> {
        let Some(session) = Self::load(dir)? else {
            return Err(format!(
                "No app session recorded at {}. Start one with `debug_app_launch`.",
                Self::path(dir)
            )
            .into());
        };

        match session.reported_pid() {
            None => Err(format!(
                "The app's pid file at {} is gone, so the recorded session (pid {}) can no longer \
                 be confirmed. The app was most likely quit outside these tools. Run \
                 `debug_app_launch` to start a new one.",
                session.pid_path(),
                session.pid
            )
            .into()),

            Some(pid) if pid != session.pid => Err(format!(
                "The app running under {} reports pid {pid}, but the recorded session is pid {}. \
                 Something launched the app outside these tools. Run `debug_app_quit` and then \
                 `debug_app_launch` to get back to a known state.",
                session.state_dir, session.pid
            )
            .into()),

            Some(pid) if !pid_is_alive(pid) => {
                let tail = session.stderr.tail();
                let note = if tail.trim().is_empty() {
                    "It wrote nothing to stderr before going away.".to_owned()
                } else {
                    format!(
                        "Its last stderr output was:\n\n```\n{}\n```",
                        tail.trim_end()
                    )
                };

                Err(format!(
                    "The app recorded as pid {pid} is no longer running — it quit or crashed. \
                     {note}\n\nRun `debug_app_launch` to start a new one."
                )
                .into())
            }

            Some(_) => Ok(session),
        }
    }

    /// Whether the recorded app is running right now.
    pub(crate) fn is_running(&self) -> bool {
        self.reported_pid()
            .is_some_and(|pid| pid == self.pid && pid_is_alive(pid))
    }

    /// Where the app writes its own process id.
    pub(crate) fn pid_path(&self) -> Utf8PathBuf {
        self.state_dir.join("pid")
    }

    /// The ASLR slide the app reported for its own main image.
    ///
    /// Read from the process that has it rather than recovered from a trace,
    /// which is both exact and the only option for a recording that attached
    /// after the app's images were already mapped.
    /// `None` for a build that does not report one.
    pub(crate) fn reported_slide(&self) -> Option<xct2cli::Slide> {
        let raw = fs::read_to_string(self.state_dir.join(SLIDE_FILE)).ok()?;

        raw.trim().parse::<u64>().ok().map(xct2cli::Slide::new)
    }

    /// The pid the app currently claims, or `None` when it has not claimed one.
    fn reported_pid(&self) -> Option<u32> {
        fs::read_to_string(self.pid_path())
            .ok()?
            .trim()
            .parse::<u32>()
            .ok()
    }
}

/// A signal one of these tools sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Signal {
    /// Ask a process to stop the way Ctrl-C would.
    Int,
    Term,
    Kill,
}

/// Signalling a process, and observing whether it is still there.
///
/// A seam, so an escalation ladder can be driven by a process that reliably
/// survives `SIGTERM`.
/// Building that fixture out of a real process means relying on shell trap
/// semantics, which turned out to be too subtle to trust: a shell told to
/// ignore `SIGTERM` still exited, and the test passed through the first rung
/// while claiming to cover the second.
pub(crate) trait Signals {
    fn send(&self, pid: u32, signal: Signal);

    fn is_alive(&self, pid: u32) -> bool;

    /// Whether `pid` is running something whose command holds `expected`.
    ///
    /// A process id is not an identity.
    /// The app or the recorder can exit leaving the record that names its pid
    /// behind, and the kernel hands that number out again — macOS wraps at
    /// five digits, so on a busy machine that is hours rather than never.
    /// Signalling on the number alone would then send `SIGTERM`, and in the
    /// escalation `SIGKILL`, to a stranger.
    ///
    /// Defaults to `true`, for fakes standing in for a process that is what it
    /// says it is.
    fn is(&self, _pid: u32, _expected: &str) -> bool {
        true
    }
}

/// Production [`Signals`]: real `kill(2)`.
pub(crate) struct RealSignals;

impl Signals for RealSignals {
    /// Asked of `ps`, which is the only thing that knows.
    ///
    /// A process that cannot be read is reported as not matching: refusing to
    /// signal degrades to the behaviour for one already gone, which is reported
    /// and harmless, where signalling wrongly is neither.
    fn is(&self, pid: u32, expected: &str) -> bool {
        let Ok(output) = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
        else {
            return false;
        };

        String::from_utf8_lossy(&output.stdout)
            .trim()
            .contains(expected)
    }

    #[cfg(unix)]
    fn send(&self, pid: u32, signal: Signal) {
        let sig = match signal {
            Signal::Int => libc::SIGINT,
            Signal::Term => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
        };

        // Best-effort: a failure means the process has already exited, which
        // `is_alive` then observes.
        unsafe {
            libc::kill(pid.cast_signed(), sig);
        }
    }

    #[cfg(not(unix))]
    fn send(&self, _pid: u32, _signal: Signal) {}

    fn is_alive(&self, pid: u32) -> bool {
        pid_is_alive(pid)
    }
}

/// Whether a process with `pid` is currently alive.
///
/// On non-unix targets, conservatively returns `true`: these tools are macOS
/// only, and a wrong `false` would report a running app as gone.
#[cfg(unix)]
pub(crate) fn pid_is_alive(pid: u32) -> bool {
    // `kill(pid, 0)` runs the kernel's permission and existence checks without
    // sending a signal: 0 => alive; EPERM => alive but not ours; ESRCH => gone.
    if unsafe { libc::kill(pid.cast_signed(), 0) } == 0 {
        return true;
    }

    io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(not(unix))]
pub(crate) fn pid_is_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
