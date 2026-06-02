//! Spawn `jp` and let the caller attach sidecars (like `sample(1)`) before
//! waiting on it.
//!
//! Two-phase: [`spawn`] returns a [`Handle`] carrying the PID and start time,
//! and the caller calls [`Handle::wait`] when ready.
//! This shape lets each profiling tool start its own profiler keyed on the PID
//! between the two phases.
//!
//! [`Handle::wait`] is bounded: a profiled `jp` that never exits on its own is
//! given a run budget (measured from spawn, so compilation is already
//! excluded), then shut down via an escalating ladder.
//! Two graceful-shutdown nudges (`SIGINT` on Unix, `Ctrl+Break` on Windows),
//! each followed by a grace window, mirror a manual double `Ctrl+C` and drive
//! jp's own graceful shutdown, which flushes the trace log and the dhat heap
//! profile.
//! A wedged process is force-killed as a backstop (`SIGKILL` on Unix, job
//! termination on Windows), which loses those artifacts.
//! How the process ended is reported in [`LaunchResult::termination`].

use std::{
    io::Read,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use camino::Utf8PathBuf;
#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::{
        Console::{CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent},
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject,
        },
        Threading::CREATE_NEW_PROCESS_GROUP,
    },
};

use crate::Error;

/// Run budget after build before we start shutting jp down.
/// Non-configurable.
pub(crate) const RUN_TIMEOUT: Duration = Duration::from_mins(1);

/// Poll interval while waiting on the child.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Time budget governing a launched process.
///
/// [`Timeouts::DEFAULT`] is the production budget.
/// Tests construct tiny values to exercise the shutdown ladder in milliseconds
/// rather than minutes.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timeouts {
    /// How long the process may run (measured from spawn) before we step in.
    pub run: Duration,
    /// Grace window after each graceful-shutdown nudge before escalating.
    pub sigint_grace: Duration,
    /// How long to wait for a force-killed process to disappear.
    pub reap_grace: Duration,
}

impl Timeouts {
    /// The production budget: a [`RUN_TIMEOUT`] run window and 5s graces.
    pub const DEFAULT: Timeouts = Timeouts {
        run: RUN_TIMEOUT,
        sigint_grace: Duration::from_secs(5),
        reap_grace: Duration::from_secs(5),
    };

    /// The default budget with an overridden run window.
    pub const fn with_run(run: Duration) -> Timeouts {
        Timeouts {
            run,
            sigint_grace: Timeouts::DEFAULT.sigint_grace,
            reap_grace: Timeouts::DEFAULT.reap_grace,
        }
    }
}

/// What process to launch.
#[derive(Debug, Clone)]
pub(crate) struct LaunchSpec {
    pub binary: Utf8PathBuf,
    pub args: Vec<String>,
    pub working_dir: Utf8PathBuf,
    pub env: Vec<(String, String)>,
}

/// How a launched process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Termination {
    /// Exited on its own within the run budget.
    Exited,
    /// Did not exit on its own, but stopped after a graceful-shutdown request
    /// (`SIGINT` on Unix, `Ctrl+Break` on Windows).
    /// The trace log and heap profile reflect a clean shutdown and should be
    /// complete.
    Graceful,
    /// Ignored the graceful-shutdown request and was force-killed (`SIGKILL` on
    /// Unix, job termination on Windows).
    /// Artifacts that depend on a clean shutdown (trace log, heap profile) may
    /// be missing.
    Forced,
}

/// Result of a completed launch.
#[derive(Debug, Clone)]
pub(crate) struct LaunchResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub wall_duration: Duration,
    pub termination: Termination,
}

impl LaunchResult {
    pub(crate) fn success(&self) -> bool {
        matches!(self.exit_code, Some(0))
    }

    /// A one-line note describing a non-natural shutdown, for the report.
    /// Returns `None` when jp exited on its own.
    pub(crate) fn note(&self) -> Option<String> {
        let secs = self.wall_duration.as_secs_f64();
        match self.termination {
            Termination::Exited => None,
            Termination::Graceful => Some(format!(
                "jp did not exit on its own and was gracefully shut down after {secs:.0}s. \
                 Results below reflect a clean shutdown and should be complete."
            )),
            Termination::Forced => Some(format!(
                "jp did not respond to graceful shutdown and was force-killed after {secs:.0}s. \
                 Results below may be partial or missing."
            )),
        }
    }
}

/// In-flight launch.
/// Holds the spawned child plus its identifying metadata.
pub(crate) struct Handle {
    child: Child,
    pid: u32,
    started_at: Instant,
    /// Kill-on-close job owning jp and its descendants, so a wedged run is torn
    /// down as a unit and can't leak background processes.
    #[cfg(windows)]
    job: Job,
}

impl Handle {
    /// PID of the launched process — for attaching profilers like `sample(1)`.
    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }

    /// Wait for the child to exit and collect its output, enforcing `timeouts`
    /// (measured from spawn).
    /// A child that overruns its run budget is shut down via the graceful →
    /// grace → graceful → grace → force-kill ladder; the outcome is recorded
    /// in [`LaunchResult::termination`].
    pub(crate) fn wait(mut self, timeouts: Timeouts) -> Result<LaunchResult, Error> {
        // Drain stdout/stderr on dedicated threads. Without this, a child that
        // fills a 64 KB pipe buffer would block on write while our poll loop
        // waits on exit — a deadlock.
        let stdout_reader = spawn_reader(self.child.stdout.take());
        let stderr_reader = spawn_reader(self.child.stderr.take());

        let termination = self.wait_or_terminate(&timeouts)?;

        // The child has exited (naturally or via our signals). `try_wait`
        // already reaped it inside `wait_until`, so this returns the cached
        // status without blocking.
        let status = self
            .child
            .wait()
            .map_err(|e| format!("Failed to reap jp: {e}"))?;

        Ok(LaunchResult {
            exit_code: status.code(),
            stdout: join_reader(stdout_reader),
            stderr: join_reader(stderr_reader),
            wall_duration: self.started_at.elapsed(),
            termination,
        })
    }

    /// Wait for natural exit within the run budget; otherwise escalate to a
    /// graceful then forceful shutdown.
    /// Returns how the process ended.
    fn wait_or_terminate(&mut self, timeouts: &Timeouts) -> Result<Termination, Error> {
        if wait_until(&mut self.child, timeouts.run)? {
            return Ok(Termination::Exited);
        }

        #[cfg(unix)]
        {
            // Two SIGINTs, mirroring a manual double Ctrl+C: the first requests
            // a graceful shutdown (flushing the trace log / heap profile), the
            // second escalates jp's own graceful→force path for wedged
            // background work.
            for _ in 0..2 {
                signal_group(self.pid, libc::SIGINT);
                if wait_until(&mut self.child, timeouts.sigint_grace)? {
                    return Ok(Termination::Graceful);
                }
            }
            signal_group(self.pid, libc::SIGKILL);
        }

        #[cfg(windows)]
        {
            // Two Ctrl+Breaks, the Windows analog of the double Ctrl+C: jp
            // catches `CTRL_BREAK_EVENT` as a graceful shutdown (see jp_cli's
            // signal handling) and flushes the trace log / heap profile. jp
            // leads its own process group (see `spawn`), so the event reaches
            // jp and its children without touching the harness's console group.
            for _ in 0..2 {
                send_ctrl_break(self.pid);
                if wait_until(&mut self.child, timeouts.sigint_grace)? {
                    return Ok(Termination::Graceful);
                }
            }
            // Terminate the whole job: killing jp alone would orphan any
            // descendants it spawned.
            self.job.terminate();
        }

        wait_until(&mut self.child, timeouts.reap_grace)?;
        Ok(Termination::Forced)
    }
}

/// Spawn the process described by `spec`.
/// Stdout and stderr are captured.
pub(crate) fn spawn(spec: &LaunchSpec) -> Result<Handle, Error> {
    let mut command = Command::new(spec.binary.as_path());
    command
        .args(&spec.args)
        .current_dir(spec.working_dir.as_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in &spec.env {
        command.env(key, value);
    }

    // Put jp in its own process group so the timeout can signal the whole
    // group (`kill(-pgid, …)`) and tear down any children that stayed in it.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }

    // Put jp in its own process group so a targeted `Ctrl+Break` reaches jp (and
    // its children) without hitting the harness's own console group.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }

    let started_at = Instant::now();
    let child = command
        .spawn()
        .map_err(|e| format!("Failed to spawn {}: {e}", spec.binary))?;
    let pid = child.id();

    // Assign jp to a kill-on-close job right after spawn. jp launches its own
    // children (MCP servers, editors) much later, so they inherit the job and
    // can be reaped as a unit.
    #[cfg(windows)]
    let job = {
        let job = Job::create()?;
        job.assign(&child)?;
        job
    };

    Ok(Handle {
        child,
        pid,
        started_at,
        #[cfg(windows)]
        job,
    })
}

/// Spawn a process and supervise it to completion under `timeouts`.
///
/// Abstracts the spawn-then-supervised-wait pair so a tool's execution phase
/// can be driven with a fake launcher in tests, independent of the sandbox and
/// build steps.
/// `on_spawn` is invoked once with the child's PID after spawn and before the
/// supervised wait, so a caller can attach a sidecar profiler (e.g.
/// `sample(1)`) keyed on the PID.
pub(crate) trait Launcher {
    fn run(
        &self,
        spec: &LaunchSpec,
        timeouts: Timeouts,
        on_spawn: &mut dyn FnMut(u32),
    ) -> Result<LaunchResult, Error>;
}

/// Production [`Launcher`]: a real [`spawn`] plus the supervised
/// [`Handle::wait`].
pub(crate) struct RealLauncher;

impl Launcher for RealLauncher {
    fn run(
        &self,
        spec: &LaunchSpec,
        timeouts: Timeouts,
        on_spawn: &mut dyn FnMut(u32),
    ) -> Result<LaunchResult, Error> {
        let handle = spawn(spec)?;
        on_spawn(handle.pid());
        handle.wait(timeouts)
    }
}

/// A [`Launcher`] for tests: returns a canned [`LaunchResult`] without touching
/// any real process.
#[cfg(test)]
pub(crate) struct MockLauncher {
    result: LaunchResult,
}

#[cfg(test)]
impl MockLauncher {
    pub(crate) fn returning(result: LaunchResult) -> Self {
        Self { result }
    }
}

#[cfg(test)]
impl Launcher for MockLauncher {
    fn run(
        &self,
        _spec: &LaunchSpec,
        _timeouts: Timeouts,
        on_spawn: &mut dyn FnMut(u32),
    ) -> Result<LaunchResult, Error> {
        on_spawn(0);
        Ok(self.result.clone())
    }
}

/// Send a `CTRL_BREAK_EVENT` to jp's process group.
/// Best-effort: a failure means the process already exited. jp leads its own
/// group (see [`spawn`]), so the event reaches jp plus any children still in
/// that group.
#[cfg(windows)]
fn send_ctrl_break(pid: u32) {
    unsafe {
        GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid);
    }
}

/// A Windows job object owning jp and the descendants it spawns.
/// Configured kill-on-close, so terminating the job (or dropping its last
/// handle) tears the whole tree down — the force-kill backstop, and a guard
/// against leaked background processes.
#[cfg(windows)]
struct Job(HANDLE);

#[cfg(windows)]
impl Job {
    fn create() -> Result<Job, Error> {
        use std::ptr;

        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() {
            return Err("Failed to create job object for jp".into());
        }

        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                ptr::from_ref(&info).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            unsafe { CloseHandle(handle) };
            return Err("Failed to configure job object for jp".into());
        }

        Ok(Job(handle))
    }

    /// Put `child` (and the descendants it later spawns) into the job.
    fn assign(&self, child: &Child) -> Result<(), Error> {
        use std::os::windows::io::AsRawHandle as _;

        let ok = unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle()) };
        if ok == 0 {
            return Err("Failed to assign jp to its job object".into());
        }
        Ok(())
    }

    /// Force-terminate every process still in the job.
    /// Best-effort: a failure means they have already exited.
    fn terminate(&self) {
        unsafe {
            TerminateJobObject(self.0, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// Poll the child until it exits or `timeout` elapses.
/// Returns `true` if the child exited (and was reaped), `false` on timeout.
fn wait_until(child: &mut Child, timeout: Duration) -> Result<bool, Error> {
    let deadline = Instant::now() + timeout;
    loop {
        if child
            .try_wait()
            .map_err(|e| format!("Failed to poll jp: {e}"))?
            .is_some()
        {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// Send `sig` to jp's process group.
/// Best-effort: a failure means the process already exited. jp is its own group
/// leader (see [`spawn`]), so the negated PID targets jp plus any children
/// still in its group.
#[cfg(unix)]
fn signal_group(pid: u32, sig: libc::c_int) {
    let pgid = libc::pid_t::from(pid.cast_signed());
    unsafe {
        libc::kill(-pgid, sig);
    }
}

#[cfg(test)]
#[path = "launch_tests.rs"]
mod tests;

/// Drain a captured pipe to completion on its own thread.
fn spawn_reader<R: Read + Send + 'static>(
    reader: Option<R>,
) -> Option<thread::JoinHandle<Vec<u8>>> {
    reader.map(|mut r| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            drop(r.read_to_end(&mut buf));
            buf
        })
    })
}

/// Join a reader thread and decode its bytes lossily.
/// A panicked or missing reader yields an empty string rather than failing the
/// whole run.
fn join_reader(handle: Option<thread::JoinHandle<Vec<u8>>>) -> String {
    handle
        .and_then(|h| h.join().ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}
