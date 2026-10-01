//! The runner that spawns real processes, driven through `process_probe` so
//! every case runs the same program on every platform.

use std::{
    env,
    io::ErrorKind,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::tempdir;
use jp_process::{
    Ended, ExitCode, ProcessRunner as _, ProcessSpec, RunnerOpts, SystemProcessRunner, Watch,
};
use tokio_util::sync::CancellationToken;

const PROBE: &str = env!("CARGO_BIN_EXE_process_probe");

/// Well under anything a probe asked to sleep would take: a run finishing
/// inside it was stopped rather than left to finish.
const PROMPTLY: Duration = Duration::from_secs(10);

/// Run the probe with `steps` in `dir`.
fn probe(steps: &[&str], dir: &Utf8Path) -> ProcessSpec {
    ProcessSpec::new(PROBE, steps.iter().copied(), dir)
}

/// The package directory, as a working directory that always exists.
fn here() -> Utf8PathBuf {
    env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR: run tests via `cargo test` or `cargo nextest`")
        .into()
}

#[test]
fn collects_both_streams_and_the_exit_code() {
    let finished = SystemProcessRunner
        .execute(
            &probe(&["out:one", "err:two", "out:three", "exit:3"], &here()),
            &Watch::default(),
        )
        .unwrap();

    assert_eq!(finished.ended, Ended::Exited);
    assert_eq!(finished.output.stdout, "one\nthree\n");
    assert_eq!(finished.output.stderr, "two\n");
    assert_eq!(finished.output.status, ExitCode::from_code(3));
}

#[test]
fn writes_stdin_and_closes_it() {
    let output = SystemProcessRunner
        .run_with_env_and_stdin(PROBE, &["stdin"], &here(), &[], Some("hello\nworld"))
        .unwrap();

    assert_eq!(output.stdout, "hello\nworld");
    assert!(output.success());
}

#[test]
fn sets_variables_on_top_of_the_environment() {
    let output = SystemProcessRunner
        .run_with_env(PROBE, &["env:JP_PROCESS_PROBE"], &here(), &[(
            "JP_PROCESS_PROBE",
            "42",
        )])
        .unwrap();

    assert_eq!(output.stdout, "42\n");
}

/// A clean environment is for sandboxed processes, which must not see what the
/// parent holds in its own: `PATH` is set in every environment a test runs in.
#[test]
fn a_clean_environment_holds_only_the_variables_given() {
    let opts = |clean_env| RunnerOpts {
        env: &[("JP_PROCESS_PROBE", "42")],
        clean_env,
        ..RunnerOpts::default()
    };
    let steps = ["env:PATH", "env:JP_PROCESS_PROBE"];

    let inherited = SystemProcessRunner
        .run_with_opts(PROBE, &steps, &here(), &opts(false))
        .unwrap();
    let clean = SystemProcessRunner
        .run_with_opts(PROBE, &steps, &here(), &opts(true))
        .unwrap();

    assert!(
        !inherited.stdout.starts_with("<unset>"),
        "{}",
        inherited.stdout
    );
    assert_eq!(clean.stdout, "<unset>\n42\n");
}

#[test]
fn runs_in_the_directory_given() {
    let dir = tempdir().unwrap();

    let output = SystemProcessRunner
        .run(PROBE, &["cwd"], dir.path())
        .unwrap();

    assert_eq!(
        std::fs::canonicalize(output.stdout.trim_end()).unwrap(),
        std::fs::canonicalize(dir.path()).unwrap()
    );
}

/// A program that cannot be started is an error, not a run that failed.
#[test]
fn a_missing_program_fails_to_start() {
    let error = SystemProcessRunner
        .run("jp-process-no-such-program", &[], &here())
        .unwrap_err();

    assert_eq!(error.kind(), ErrorKind::NotFound);
}

#[test]
fn output_that_is_not_utf8_is_decoded_lossily() {
    let output = SystemProcessRunner
        .run(PROBE, &["bytes", "out:after"], &here())
        .unwrap();

    assert_eq!(output.stdout, "\u{fffd}\nafter\n");
}

#[test]
fn each_line_of_stderr_is_passed_on_as_it_arrives() {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);

    let finished = SystemProcessRunner
        .execute(
            &probe(&["err:first", "out:unseen", "err:second"], &here()),
            &Watch {
                stderr_lines: Some(Arc::new(move |line: &str| {
                    sink.lock().unwrap().push(line.to_owned());
                })),
                ..Watch::default()
            },
        )
        .unwrap();

    assert_eq!(*lines.lock().unwrap(), ["first", "second"]);
    assert_eq!(finished.output.stderr, "first\nsecond\n");
}

/// A matching line stops a process that would otherwise run on.
#[test]
fn a_matching_line_stops_the_process() {
    let started = Instant::now();

    let finished = SystemProcessRunner
        .run_until(
            PROBE,
            &["out:working", "err:ready", "sleep:30000", "out:too late"],
            &here(),
            Arc::new(|line: &str| line == "ready"),
            Duration::ZERO,
        )
        .unwrap();

    assert!(started.elapsed() < PROMPTLY, "{:?}", started.elapsed());
    assert_eq!(finished.ended, Ended::Stopped);
    assert_eq!(finished.output.stdout, "working\n");
    assert_eq!(finished.output.stderr, "ready\n");
    assert!(!finished.output.success());
}

#[test]
fn cancelling_stops_the_process() {
    let cancellation = CancellationToken::new();
    let cancel = cancellation.clone();
    let started = Instant::now();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        cancel.cancel();
    });

    let finished = SystemProcessRunner
        .execute(&probe(&["sleep:30000"], &here()), &Watch {
            cancellation: Some(cancellation),
            ..Watch::default()
        })
        .unwrap();

    assert!(started.elapsed() < PROMPTLY, "{:?}", started.elapsed());
    assert_eq!(finished.ended, Ended::Cancelled);
}

/// A process can leave its streams open to one it started.
/// Stopping it returns at once rather than waiting for that one to exit too.
#[test]
fn a_stopped_process_does_not_wait_for_what_it_started() {
    let started = Instant::now();

    let finished = SystemProcessRunner
        .run_until(
            PROBE,
            &["hold:6000", "out:holding", "sleep:30000"],
            &here(),
            Arc::new(|line: &str| line == "holding"),
            Duration::ZERO,
        )
        .unwrap();

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(finished.ended, Ended::Stopped);
}

/// Cancel a run of the probe with `steps` at the first line it prints to
/// stderr, giving it `grace` to exit.
///
/// Returns every stderr line passed on, which keeps growing for as long as the
/// runner is still reading.
fn cancel_at_first_stderr_line(
    steps: &[&str],
    own_process_group: bool,
    grace: Duration,
) -> Arc<Mutex<Vec<String>>> {
    let cancellation = CancellationToken::new();
    let cancel = cancellation.clone();
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);

    let finished = SystemProcessRunner
        .execute(
            &ProcessSpec {
                own_process_group,
                ..probe(steps, &here())
            },
            &Watch {
                stderr_lines: Some(Arc::new(move |line: &str| {
                    sink.lock().unwrap().push(line.to_owned());
                    cancel.cancel();
                })),
                cancellation: Some(cancellation),
                grace,
                ..Watch::default()
            },
        )
        .unwrap();

    assert_eq!(finished.ended, Ended::Cancelled);
    lines
}

/// The id of the process that printed `line`, as `tick PID` or `pid PID`.
#[cfg(unix)]
fn pid_in(line: &str) -> libc::pid_t {
    line.split_whitespace()
        .nth(1)
        .and_then(|pid| pid.parse().ok())
        .unwrap_or_else(|| panic!("no pid in {line:?}"))
}

/// Whether `pid` is gone within five seconds.
#[cfg(unix)]
fn exits(pid: libc::pid_t) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        // SAFETY: signal 0 delivers nothing; it only checks the process exists.
        let alive = unsafe { libc::kill(pid, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH);
        if !alive {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

/// A cancelled run stops reading what a process it started keeps printing:
/// nothing more reaches the line sink, and the process fails its next write.
#[test]
fn a_cancelled_run_stops_reading_what_it_started() {
    let lines =
        cancel_at_first_stderr_line(&["spawn:tick:30000", "sleep:30000"], false, Duration::ZERO);

    // One line can still be on its way when the run returns.
    thread::sleep(Duration::from_millis(100));
    let seen = lines.lock().unwrap().len();
    thread::sleep(Duration::from_millis(500));
    assert_eq!(lines.lock().unwrap().len(), seen, "still reading");

    #[cfg(unix)]
    {
        let pid = pid_in(&lines.lock().unwrap()[0]);
        assert!(exits(pid), "the ticking process outlived the run");
    }
}

/// A process printing progress with no newline, as a download does, is not read
/// for as long as it keeps printing: it fails its next write, and exits.
#[cfg(unix)]
#[test]
fn a_cancelled_run_stops_reading_output_without_newlines() {
    let lines = cancel_at_first_stderr_line(
        &["spawn:pid,spin:30000", "sleep:30000"],
        false,
        Duration::ZERO,
    );

    let pid = pid_in(&lines.lock().unwrap()[0]);
    assert!(exits(pid), "the spinning process outlived the run");
}

/// A process leading its own group is stopped with what it started, even when
/// that never writes again.
#[cfg(unix)]
#[test]
fn a_cancelled_group_is_stopped_whole() {
    let lines = cancel_at_first_stderr_line(
        &["spawn:pid,sleep:30000", "sleep:30000"],
        true,
        Duration::ZERO,
    );

    let pid = pid_in(&lines.lock().unwrap()[0]);
    assert!(exits(pid), "the started process outlived its group");
}

/// A group whose leader exits on the interrupt still has the members that
/// ignored it killed once the grace runs out.
#[cfg(unix)]
#[test]
fn a_cancelled_group_outlives_its_leader_only_for_the_grace() {
    let lines = cancel_at_first_stderr_line(
        &[
            "interrupt-exits",
            "spawn:ignore-interrupt,pid,sleep:30000",
            "sleep:30000",
        ],
        true,
        Duration::from_millis(500),
    );

    let pid = pid_in(&lines.lock().unwrap()[0]);
    assert!(exits(pid), "the started process outlived its group");
}

/// A group whose leader exited before the run was cancelled is still stopped
/// whole.
#[cfg(unix)]
#[test]
fn a_cancelled_group_is_stopped_after_its_leader_exited() {
    let lines =
        cancel_at_first_stderr_line(&["spawn:sleep:300,pid,sleep:30000"], true, Duration::ZERO);

    let pid = pid_in(&lines.lock().unwrap()[0]);
    assert!(exits(pid), "the started process outlived its group");
}

/// A process asked to stop with a grace period is interrupted first, as Ctrl-C
/// would, and gets to exit on its own.
#[cfg(unix)]
#[test]
fn a_grace_period_interrupts_before_it_kills() {
    let started = Instant::now();

    let finished = SystemProcessRunner
        .run_until(
            PROBE,
            &["interrupt-exits", "out:ready", "sleep:30000"],
            &here(),
            Arc::new(|line: &str| line == "ready"),
            Duration::from_secs(20),
        )
        .unwrap();

    assert!(started.elapsed() < PROMPTLY, "{:?}", started.elapsed());
    assert_eq!(finished.output.status, ExitCode::from_code(42));
}

/// A process that ignores the interrupt is killed once its grace runs out.
#[cfg(unix)]
#[test]
fn a_process_that_ignores_the_interrupt_is_killed_after_its_grace() {
    let grace = Duration::from_millis(300);
    let started = Instant::now();

    let finished = SystemProcessRunner
        .run_until(
            PROBE,
            &["ignore-interrupt", "out:ready", "sleep:30000"],
            &here(),
            Arc::new(|line: &str| line == "ready"),
            grace,
        )
        .unwrap();

    let elapsed = started.elapsed();
    assert!(elapsed >= grace, "{elapsed:?}");
    assert!(elapsed < PROMPTLY, "{elapsed:?}");
    assert_eq!(finished.ended, Ended::Stopped);
    assert_eq!(finished.output.status.code(), None);
}

/// The process id and process group id a probe reports under `spec`.
#[cfg(unix)]
fn ids(spec: &ProcessSpec) -> (String, String) {
    let output = SystemProcessRunner
        .execute(spec, &Watch::default())
        .unwrap()
        .output;
    let ids: Vec<&str> = output.stdout.split_whitespace().collect();
    assert_eq!(ids.len(), 2, "{}", output.stdout);
    (ids[0].to_owned(), ids[1].to_owned())
}

/// A process asked to lead a group of its own does, so a Ctrl-C at the terminal
/// reaches whoever started it and not the process.
#[cfg(unix)]
#[test]
fn a_process_can_lead_a_group_of_its_own() {
    let (pid, group) = ids(&ProcessSpec {
        own_process_group: true,
        ..probe(&["group"], &here())
    });

    assert_eq!(pid, group, "the process does not lead its group");
}

/// By default a process shares this one's group, so a Ctrl-C reaches it too.
#[cfg(unix)]
#[test]
fn a_process_shares_this_ones_group_by_default() {
    let (pid, group) = ids(&probe(&["group"], &here()));

    assert_ne!(pid, group, "the process leads a group of its own");
}
