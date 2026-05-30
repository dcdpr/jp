use std::time::Duration;

use super::*;

/// Tiny budget so the escalation ladder runs in well under a second.
/// `run` is generous enough that an immediately-exiting child is reliably
/// classified as `Exited` even under load, while the loop/sleep fixtures never
/// finish within it.
fn fast_timeouts() -> Timeouts {
    Timeouts {
        run: Duration::from_millis(300),
        sigint_grace: Duration::from_millis(150),
        reap_grace: Duration::from_millis(500),
    }
}

/// Spawn `/bin/sh -c <script>` under the launch harness.
#[cfg(unix)]
fn sh(script: &str) -> Handle {
    spawn(&LaunchSpec {
        binary: "/bin/sh".into(),
        args: vec!["-c".to_owned(), script.to_owned()],
        working_dir: ".".into(),
        env: vec![],
    })
    .unwrap()
}

/// Spawn `cmd /C <command>` under the launch harness.
#[cfg(windows)]
fn cmd(command: &str) -> Handle {
    spawn(&LaunchSpec {
        binary: "cmd".into(),
        args: vec!["/C".to_owned(), command.to_owned()],
        working_dir: ".".into(),
        env: vec![],
    })
    .unwrap()
}

#[cfg(unix)]
#[test]
fn exits_naturally_within_budget() {
    let result = sh("exit 3").wait(fast_timeouts()).unwrap();
    assert_eq!(result.termination, Termination::Exited);
    assert_eq!(result.exit_code, Some(3));
    assert!(result.note().is_none());
}

#[cfg(unix)]
#[test]
fn graceful_shutdown_on_sigint() {
    // Default SIGINT disposition: the first SIGINT terminates `sleep 30` long
    // before the run budget could ever let it finish. Reaching `Graceful`
    // proves our SIGINT landed.
    let result = sh("sleep 30").wait(fast_timeouts()).unwrap();
    assert_eq!(result.termination, Termination::Graceful);
    // Killed by signal, so there is no exit code.
    assert_eq!(result.exit_code, None);
}

#[cfg(unix)]
#[test]
fn force_kill_when_sigint_ignored() {
    // The shell ignores INT and survives; the transient `sleep` dies on the
    // group signal but the loop respawns it, so only SIGKILL stops the group.
    let result = sh(r#"trap "" INT; while :; do sleep 1; done"#)
        .wait(fast_timeouts())
        .unwrap();
    assert_eq!(result.termination, Termination::Forced);
    assert_eq!(result.exit_code, None);
}

#[cfg(unix)]
#[test]
fn captures_output_even_when_killed() {
    // Output written before the process blocks must survive the kill: the
    // reader threads drain the pipes independently of the exit.
    let result = sh("echo out; echo err 1>&2; sleep 30")
        .wait(fast_timeouts())
        .unwrap();
    assert_eq!(result.termination, Termination::Graceful);
    assert_eq!(result.stdout.trim(), "out");
    assert_eq!(result.stderr.trim(), "err");
}

#[cfg(windows)]
#[test]
fn exits_naturally_within_budget() {
    let result = cmd("exit 3").wait(fast_timeouts()).unwrap();
    assert_eq!(result.termination, Termination::Exited);
    assert_eq!(result.exit_code, Some(3));
    assert!(result.note().is_none());
}

#[cfg(windows)]
#[test]
fn job_terminate_kills_assigned_process() {
    use std::{
        process::{Command, Stdio},
        thread,
        time::Instant,
    };

    // A child that would otherwise ping for ~30s. Assign it to a fresh job,
    // terminate the job, and confirm the process is gone — the force-kill
    // backstop the run timeout relies on.
    let mut child = Command::new("ping")
        .args(["-n", "30", "127.0.0.1"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let job = Job::create().unwrap();
    job.assign(&child).unwrap();
    job.terminate();

    // Job termination is asynchronous; poll briefly for the child to exit.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "job-terminated child did not exit"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn note_present_only_for_non_natural_exit() {
    let result = |termination| LaunchResult {
        exit_code: Some(0),
        stdout: String::new(),
        stderr: String::new(),
        wall_duration: Duration::from_secs(1),
        termination,
    };
    assert!(result(Termination::Exited).note().is_none());
    assert!(result(Termination::Graceful).note().is_some());
    assert!(result(Termination::Forced).note().is_some());
}
