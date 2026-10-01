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

use std::process::Child;
#[cfg(unix)]
use std::{io, mem};

use tracing::debug;

#[cfg(windows)]
pub(crate) use self::job::resume_suspended;

/// A spawned plugin and everything it starts.
pub(crate) struct ProcessTree {
    pid: u32,

    #[cfg(windows)]
    job: Option<job::Job>,
}

impl ProcessTree {
    /// Take ownership of the tree `child` leads.
    ///
    /// On Unix, `child` must have been spawned as the leader of its own process
    /// group.
    /// On Windows, it must have been spawned suspended, and be resumed only
    /// after this returns.
    pub(crate) fn new(child: &Child) -> Self {
        Self {
            pid: child.id(),

            #[cfg(windows)]
            job: job::Job::assign(child),
        }
    }

    /// The pid of the process JP spawned.
    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }

    /// Wait for the spawned process to exit, kill whatever it left running, and
    /// reap it.
    ///
    /// The process is reaped last: until then its pid, and with it the process
    /// group id, cannot be reused, so the kill cannot reach an unrelated group.
    #[cfg(unix)]
    pub(crate) fn finish(&self, child: &mut Child) {
        self.wait_exited();
        self.terminate();
        drop(child.wait());
    }

    /// Wait for the spawned process to exit, kill whatever it left running, and
    /// reap it.
    ///
    /// A job is addressed by its handle rather than an id, so the order does
    /// not matter here.
    #[cfg(windows)]
    pub(crate) fn finish(&self, child: &mut Child) {
        drop(child.wait());
        self.terminate();
    }

    /// Block until the spawned process has exited, leaving it unreaped.
    #[cfg(unix)]
    fn wait_exited(&self) {
        loop {
            // SAFETY: an all-zero `siginfo_t` is a valid value for `waitid` to
            // overwrite. `WNOWAIT` leaves the process waitable, so `Child::wait`
            // still reaps it afterwards.
            let waited = unsafe {
                let mut info: libc::siginfo_t = mem::zeroed();
                libc::waitid(
                    libc::P_PID,
                    libc::id_t::from(self.pid),
                    &raw mut info,
                    libc::WEXITED | libc::WNOWAIT,
                )
            };

            if waited == 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                return;
            }
        }
    }

    /// Kill every process in the tree that is still running.
    #[cfg(unix)]
    pub(crate) fn terminate(&self) {
        let group = -libc::pid_t::from(self.pid.cast_signed());

        // SAFETY: `kill` has no memory-safety preconditions. The group is the
        // one the spawn created for the plugin; its id stays reserved while the
        // plugin, or any process in the group, has not been reaped.
        let sent = unsafe { libc::kill(group, libc::SIGKILL) } == 0;
        debug!(pid = self.pid, sent, "Killed the plugin's process group.");
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

#[cfg(windows)]
mod job {
    use std::{io, mem, os::windows::io::AsRawHandle as _, process::Child, ptr};

    use tracing::warn;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            JobObjects::{AssignProcessToJobObject, CreateJobObjectW, TerminateJobObject},
            Threading::{
                OpenProcess, OpenThread, PROCESS_TERMINATE, ResumeThread, THREAD_SUSPEND_RESUME,
                TerminateProcess,
            },
        },
    };

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
