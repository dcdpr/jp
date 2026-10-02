//! The process tree of a running plugin.
//!
//! JP owns the plugin's process tree for the lifetime of the invocation, not
//! only the process it spawned: terminating a plugin terminates everything it
//! started, so a worker a shell-script plugin launched cannot outlive it, keep
//! a port bound, or hold the pipes JP waits on.
//!
//! On Unix the plugin leads its own process group, which the spawn sets up
//! (`process_group(0)`), so the group's id is the plugin's pid.
//! On Windows the plugin is spawned suspended (`CREATE_SUSPENDED`), assigned to
//! a job object, and only then resumed with `resume_suspended`, so nothing it
//! starts can escape the job.
//! A descendant that deliberately detaches from the group or the job is not
//! covered.
//!
//! See: `docs/rfd/072-command-plugin-system.md`, "Shutdown".

#[cfg(windows)]
use std::sync::Arc;
#[cfg(unix)]
use std::{io, mem};
use std::{
    process::Child,
    sync::Mutex,
    thread::{self, JoinHandle},
};

use tracing::{debug, warn};

#[cfg(windows)]
pub(crate) use self::job::resume_suspended;

/// A spawned plugin and everything it starts.
pub(crate) struct ProcessTree {
    pid: u32,

    /// The thread that kills what is left of the tree once the spawned process
    /// exits.
    ///
    /// `None` once `finish` has joined it, or when it could not be started.
    watcher: Mutex<Option<JoinHandle<()>>>,

    #[cfg(windows)]
    job: Option<Arc<job::Job>>,
}

impl ProcessTree {
    /// Take ownership of the tree `child` leads.
    ///
    /// What is left of the tree is killed as soon as `child` exits, before
    /// `finish` is called, so a worker that holds one of the plugin's pipes
    /// cannot keep it open after the plugin is gone.
    ///
    /// On Unix, `child` must have been spawned as the leader of its own process
    /// group.
    /// On Windows, it must have been spawned suspended, and be resumed only
    /// after this returns.
    /// On both, `child` must be reaped through `finish` and nothing else.
    #[cfg(unix)]
    pub(crate) fn new(child: &Child) -> Self {
        let pid = child.id();

        Self {
            pid,
            watcher: Mutex::new(watch(move || kill_group_on_exit(pid))),
        }
    }

    /// Take ownership of the tree `child` leads.
    ///
    /// What is left of the tree is killed as soon as `child` exits, before
    /// `finish` is called, so a worker that holds one of the plugin's pipes
    /// cannot keep it open after the plugin is gone.
    ///
    /// On Unix, `child` must have been spawned as the leader of its own process
    /// group.
    /// On Windows, it must have been spawned suspended, and be resumed only
    /// after this returns.
    /// On both, `child` must be reaped through `finish` and nothing else.
    #[cfg(windows)]
    pub(crate) fn new(child: &Child) -> Self {
        let job = job::Job::assign(child).map(Arc::new);

        // Without a job there is nothing to kill but the plugin, which has
        // already exited by the time the watcher would act.
        let watcher = job.as_ref().and_then(|job| {
            let process = job::ExitWaiter::new(child)?;
            let job = Arc::clone(job);

            watch(move || {
                if process.wait() {
                    job.terminate();
                }
            })
        });

        Self {
            pid: child.id(),
            watcher: Mutex::new(watcher),
            job,
        }
    }

    /// The pid of the process JP spawned.
    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }

    /// Wait for the spawned process to exit, kill whatever it left running, and
    /// reap it.
    ///
    /// On Unix the process is reaped last: until then its pid, and with it the
    /// process group id, cannot be reused, so the kill cannot reach an
    /// unrelated group.
    pub(crate) fn finish(&self, child: &mut Child) {
        let watcher = self.watcher.lock().expect("watcher lock poisoned").take();
        match watcher {
            Some(watcher) => drop(watcher.join()),
            // Nothing watched for the exit, so it is waited for here.
            None => self.terminate_after_exit(child),
        }

        drop(child.wait());
    }

    /// Wait for the spawned process to exit, then kill what is left of its
    /// group, leaving it unreaped.
    #[cfg(unix)]
    fn terminate_after_exit(&self, _child: &mut Child) {
        kill_group_on_exit(self.pid);
    }

    /// Wait for the spawned process to exit, then kill what is left of its job.
    ///
    /// A job is addressed by its handle rather than an id, so reaping first
    /// does not matter here.
    #[cfg(windows)]
    fn terminate_after_exit(&self, child: &mut Child) {
        drop(child.wait());
        self.terminate();
    }

    /// Kill every process in the tree that is still running.
    #[cfg(unix)]
    pub(crate) fn terminate(&self) {
        kill_group(self.pid);
    }

    /// Kill every process in the tree that is still running.
    #[cfg(windows)]
    pub(crate) fn terminate(&self) {
        match &self.job {
            Some(job) => {
                job.terminate();
                debug!(pid = self.pid, "Terminated the plugin's job object.");
            }
            None => {
                job::terminate_process(self.pid);
                debug!(
                    pid = self.pid,
                    "Terminated the plugin, which has no job object."
                );
            }
        }
    }
}

/// Start the thread that cleans up after the plugin once it exits.
///
/// `None` when the thread could not be started, in which case `finish` does the
/// same work once it is called.
fn watch(cleanup: impl FnOnce() + Send + 'static) -> Option<JoinHandle<()>> {
    thread::Builder::new()
        .name("plugin-watcher".to_owned())
        .spawn(cleanup)
        .inspect_err(|error| warn!(%error, "Could not watch the plugin for exiting."))
        .ok()
}

/// Block until `pid` has exited, then kill what is left of its process group.
///
/// `pid` is left unreaped, so the group id stays reserved for the kill.
/// If `pid` cannot be waited on, most likely because it was already reaped, the
/// group id may belong to someone else by now, and nothing is killed.
#[cfg(unix)]
fn kill_group_on_exit(pid: u32) {
    loop {
        // SAFETY: an all-zero `siginfo_t` is a valid value for `waitid` to
        // overwrite. `WNOWAIT` leaves the process waitable, so `Child::wait`
        // still reaps it afterwards.
        let waited = unsafe {
            let mut info: libc::siginfo_t = mem::zeroed();
            libc::waitid(
                libc::P_PID,
                libc::id_t::from(pid),
                &raw mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };

        if waited == 0 {
            break;
        }

        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            debug!(%error, pid, "Could not wait on the plugin; not killing its group.");
            return;
        }
    }

    kill_group(pid);
}

/// Kill every process in the group `pid` leads.
#[cfg(unix)]
fn kill_group(pid: u32) {
    let group = -libc::pid_t::from(pid.cast_signed());

    // SAFETY: `kill` has no memory-safety preconditions. The group is the one
    // the spawn created for the plugin; its id stays reserved while the plugin,
    // or any process in the group, has not been reaped.
    let sent = unsafe { libc::kill(group, libc::SIGKILL) } == 0;
    debug!(pid, sent, "Killed the plugin's process group.");
}

#[cfg(windows)]
mod job {
    use std::{
        io, mem,
        os::windows::io::{AsHandle as _, AsRawHandle as _, OwnedHandle},
        process::Child,
        ptr,
    };

    use tracing::{debug, warn};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            JobObjects::{AssignProcessToJobObject, CreateJobObjectW, TerminateJobObject},
            Threading::{
                INFINITE, OpenProcess, OpenThread, PROCESS_TERMINATE, ResumeThread,
                THREAD_SUSPEND_RESUME, TerminateProcess, WaitForSingleObject,
            },
        },
    };

    /// A handle to the plugin process of its own, to wait on from another
    /// thread while the `Child` stays with its owner.
    pub(super) struct ExitWaiter(OwnedHandle);

    impl ExitWaiter {
        pub(super) fn new(child: &Child) -> Option<Self> {
            child
                .as_handle()
                .try_clone_to_owned()
                .inspect_err(|error| warn!(%error, "Could not watch the plugin for exiting."))
                .ok()
                .map(Self)
        }

        /// Block until the process has exited.
        ///
        /// Returns `false` when it could not be waited on, in which case it may
        /// still be running.
        pub(super) fn wait(&self) -> bool {
            // SAFETY: the handle is open for as long as `self` is.
            let waited = unsafe { WaitForSingleObject(self.0.as_raw_handle(), INFINITE) };
            if waited != WAIT_OBJECT_0 {
                debug!(
                    waited,
                    "Could not wait on the plugin; not terminating its job."
                );
                return false;
            }

            true
        }
    }

    /// An owned job object handle, closed on drop.
    pub(super) struct Job(HANDLE);

    // SAFETY: a job object handle is a kernel handle, valid on any thread. It is
    // only passed to `TerminateJobObject` and `CloseHandle`, which are safe to
    // call from any thread, and it is closed exactly once, on drop.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        /// Create a job and assign `child` to it.
        ///
        /// A process `child` starts before the assignment lands is not in the
        /// job, which is why the plugin is spawned suspended.
        pub(super) fn assign(child: &Child) -> Option<Self> {
            // SAFETY: both arguments may be null, which asks for the default
            // security and an unnamed job.
            let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
            if handle.is_null() {
                warn!("Could not create a job object for the plugin.");
                return None;
            }

            let job = Self(handle);

            // SAFETY: both handles are valid: the job was just created, and the
            // child's handle lives as long as the `Child` it is borrowed from.
            let assigned = unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle()) };
            if assigned == 0 {
                warn!("Could not assign the plugin to its job object.");
                return None;
            }

            Some(job)
        }

        pub(super) fn terminate(&self) {
            // SAFETY: the handle is valid until drop.
            unsafe { TerminateJobObject(self.0, 1) };
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle is valid, and closed only here.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// Terminate one process, when there is no job to terminate.
    pub(super) fn terminate_process(pid: u32) {
        // SAFETY: the handle is checked before use and closed after it.
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if !handle.is_null() {
                TerminateProcess(handle, 1);
                CloseHandle(handle);
            }
        }
    }

    /// Resume the only thread of a process created with `CREATE_SUSPENDED`.
    ///
    /// `std::process::Child` does not expose the primary thread handle, so the
    /// thread is found through a snapshot of the system's threads.
    /// A suspended process that has not run yet has exactly one.
    pub(crate) fn resume_suspended(pid: u32) -> io::Result<()> {
        // SAFETY: every handle is checked before use and closed once; `entry`
        // is a plain C struct whose size field is set as the API requires.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }

            let mut entry = THREADENTRY32 {
                dwSize: u32::try_from(mem::size_of::<THREADENTRY32>()).unwrap_or(u32::MAX),
                ..THREADENTRY32::default()
            };

            let mut thread_id = None;
            let mut more = Thread32First(snapshot, &raw mut entry) != 0;
            while more {
                if entry.th32OwnerProcessID == pid {
                    thread_id = Some(entry.th32ThreadID);
                    break;
                }
                more = Thread32Next(snapshot, &raw mut entry) != 0;
            }
            CloseHandle(snapshot);

            let thread_id =
                thread_id.ok_or_else(|| io::Error::other("the plugin has no thread to resume"))?;

            let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id);
            if thread.is_null() {
                return Err(io::Error::last_os_error());
            }

            let previous = ResumeThread(thread);
            let error = io::Error::last_os_error();
            CloseHandle(thread);

            if previous == u32::MAX {
                return Err(error);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
#[path = "process_tree_tests.rs"]
mod tests;
