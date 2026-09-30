use std::{
    fs,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use camino::Utf8Path;
use camino_tempfile::tempdir;

use super::*;

/// Whether `pid` names a process that is still running.
///
/// On Unix a zombie counts as running, so callers wait for it to be reaped.
#[cfg(unix)]
fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(libc::pid_t::from(pid.cast_signed()), 0) == 0 }
}

/// Whether `pid` names a process that is still running.
///
/// A terminated process whose handle someone still holds exists, but no longer
/// reports `STILL_ACTIVE`, so it counts as gone.
#[cfg(windows)]
fn alive(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, STILL_ACTIVE},
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };

    // SAFETY: the handle is checked before use and closed after it.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        ok != 0 && code.cast_signed() == STILL_ACTIVE
    }
}

/// Wait up to `limit` for `pid` to be gone: a killed process is reaped by its
/// parent, which for an orphan takes a moment.
fn gone_within(pid: u32, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if !alive(pid) {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

/// A plugin stand-in that does nothing for a minute, set up the way
/// `spawn_plugin` sets up a plugin: on Unix as the leader of its own group.
fn idle() -> Child {
    #[cfg(unix)]
    let mut command = {
        use std::os::unix::process::CommandExt as _;

        let mut command = Command::new("sleep");
        command.arg("60").process_group(0);
        command
    };

    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("ping");
        command.args(["-n", "60", "127.0.0.1"]);
        command
    };

    command
        .stdout(Stdio::null())
        .spawn()
        .expect("the idle command is available")
}

/// A plugin stand-in that starts a background worker, writes the worker's pid
/// to `pid_file`, and waits for it.
fn with_worker(pid_file: &Utf8Path) -> Child {
    #[cfg(unix)]
    let mut command = {
        use std::os::unix::process::CommandExt as _;

        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(format!("sleep 60 & echo $! > {pid_file}; wait"))
            .process_group(0);
        command
    };

    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("powershell");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "$p = Start-Process -FilePath ping -ArgumentList '-n','60','127.0.0.1' \
                 -NoNewWindow -PassThru -RedirectStandardOutput '{pid_file}.out'; Set-Content \
                 -Path '{pid_file}' -Value $p.Id -Encoding ascii; Wait-Process -Id $p.Id"
            ),
        ]);
        command
    };

    command
        .stdout(Stdio::null())
        .spawn()
        .expect("the shell is available")
}

/// The pid `with_worker` wrote, once it has written it.
fn worker_pid(pid_file: &Utf8Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(pid) = fs::read_to_string(pid_file)
            && let Ok(pid) = pid.trim().parse::<u32>()
        {
            return pid;
        }
        assert!(Instant::now() < deadline, "the worker never started");
        thread::sleep(Duration::from_millis(20));
    }
}

/// A worker a shell-script plugin started in the background dies with the
/// plugin, rather than keeping a port or a pipe open after it.
#[test]
fn terminating_the_tree_kills_what_the_plugin_started() {
    let tmp = tempdir().unwrap();
    let pid_file = tmp.path().join("worker.pid");

    let mut child = with_worker(&pid_file);
    let tree = ProcessTree::new(&child);

    let worker = worker_pid(&pid_file);
    assert!(
        alive(worker),
        "the worker is running before the tree is killed"
    );

    tree.terminate();

    child.wait().unwrap();
    assert!(
        gone_within(worker, Duration::from_secs(10)),
        "the worker died with its plugin"
    );
}

/// Killing only the spawned process would leave the worker running, which is
/// what this module exists to prevent; the group or the job is the unit, and a
/// process outside it is not touched.
#[test]
fn a_process_outside_the_tree_is_left_alone() {
    let mut plugin = idle();
    let mut bystander = idle();

    ProcessTree::new(&plugin).terminate();
    plugin.wait().unwrap();

    assert!(alive(bystander.id()), "another tree is not touched");

    bystander.kill().unwrap();
    bystander.wait().unwrap();
}

/// The job is what `terminate` kills; without it only the plugin itself would
/// go, and its workers would outlive it.
#[cfg(windows)]
#[test]
fn the_plugin_is_assigned_to_a_job() {
    let mut child = idle();
    let tree = ProcessTree::new(&child);

    assert!(
        tree.job.is_some(),
        "the plugin was assigned to a job object"
    );

    tree.terminate();
    child.wait().unwrap();
}
