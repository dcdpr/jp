use std::{
    fs,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use camino::{Utf8Path, Utf8PathBuf};

use super::{
    Artifact, PENDING_WINDOW, RETENTION_WINDOW, Recording, Scope, Stop, Tier, archive_stream,
    enforce_limits, is_recording, parse_tiers, pending, profiles_dir, record_args, run_issue_lines,
    run_issues, stop, streams, sweep,
};
use crate::debug_app::session::{Signal, Signals};

/// A [`Signals`] that records what it was sent and dies on one nominated
/// signal.
///
/// Only a fake can assert that `SIGINT` was the *only* signal sent, which is
/// the property that matters: `xctrace` writes the bundle out on its way, and a
/// recorder that is killed leaves one nothing can open.
struct Recorded {
    sent: Mutex<Vec<Signal>>,
    alive: AtomicBool,
    dies_on: Option<Signal>,
}

impl Recorded {
    fn dying_on(dies_on: Signal) -> Self {
        Self {
            sent: Mutex::new(Vec::new()),
            alive: AtomicBool::new(true),
            dies_on: Some(dies_on),
        }
    }

    /// A recorder that never exits, whatever it is sent.
    fn deaf() -> Self {
        Self {
            sent: Mutex::new(Vec::new()),
            alive: AtomicBool::new(true),
            dies_on: None,
        }
    }

    /// A recorder that was gone before anything was sent.
    fn gone() -> Self {
        Self {
            sent: Mutex::new(Vec::new()),
            alive: AtomicBool::new(false),
            dies_on: None,
        }
    }

    fn sent(&self) -> Vec<Signal> {
        self.sent.lock().unwrap().clone()
    }
}

impl Signals for Recorded {
    fn send(&self, _pid: u32, signal: Signal) {
        self.sent.lock().unwrap().push(signal);
        if self.dies_on == Some(signal) {
            self.alive.store(false, Ordering::SeqCst);
        }
    }

    fn is_alive(&self, _pid: u32) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
}

/// Above macOS's default maximum pid, so no process can hold it.
const DEAD_PID: u32 = 4_000_000;

fn recording(id: &str, recorder_pid: u32) -> Recording {
    Recording {
        id: id.to_owned(),
        tiers: vec![Tier::Sampling],
        scope: Scope::Attach(31657),
        recorder_pid,
        started_unix: super::unix_seconds(),
        stopped_unix: None,
        target: None,
    }
}

/// Put a recording on disk, bundle and all, as a live bracket would leave it.
fn on_disk(dir: &Utf8Path, recording: &Recording) {
    fs::create_dir_all(recording.bundle(dir).join("corespace")).unwrap();
    fs::write(
        recording.bundle(dir).join("corespace/data"),
        "PATH=/usr/bin ANTHROPIC_API_KEY=secret",
    )
    .unwrap();
    fs::write(recording.log(dir), "Ctrl-C to stop the recording\n").unwrap();
    recording.store(dir).unwrap();
}

fn temp() -> (camino_tempfile::Utf8TempDir, Utf8PathBuf) {
    let workspace = camino_tempfile::tempdir().unwrap();
    let dir = workspace.path().to_owned();

    (workspace, dir)
}

/// This argument vector is the contract with `xctrace`, and every part of it
/// was arrived at the hard way, so it is pinned exactly.
#[test]
fn record_args_attach_to_one_process() {
    assert_eq!(
        record_args(
            Utf8Path::new("/repo/tmp/profiles/p.trace"),
            &[Tier::Sampling],
            Scope::Attach(31657)
        ),
        vec![
            "xctrace",
            "record",
            "--instrument",
            "Time Profiler",
            "--attach",
            "31657",
            "--output",
            "/repo/tmp/profiles/p.trace",
        ]
    );
}

/// The system-wide form, which is the only one that can cover an app's own
/// startup and the only one that costs minutes to read back.
#[test]
fn record_args_take_the_whole_machine_with_no_process_to_attach_to() {
    assert_eq!(
        record_args(
            Utf8Path::new("/repo/tmp/profiles/p.trace"),
            &[Tier::Sampling, Tier::Allocations],
            Scope::System
        ),
        vec![
            "xctrace",
            "record",
            "--instrument",
            "Time Profiler",
            "--instrument",
            "Allocations",
            "--all-processes",
            "--output",
            "/repo/tmp/profiles/p.trace",
        ]
    );
}

/// A template produces a bundle whose export fails with "Document Missing
/// Template Error" on Xcode 26, and `--no-prompt` lets a recording abort about
/// 34ms in.
/// Neither absence is incidental.
#[test]
fn record_args_use_no_template_and_never_prompt_free() {
    let args = record_args(
        Utf8Path::new("/repo/tmp/profiles/p.trace"),
        &[Tier::Sampling],
        Scope::Attach(1),
    );

    assert!(!args.contains(&"--template".to_owned()), "{args:?}");
    assert!(!args.contains(&"--no-prompt".to_owned()), "{args:?}");
}

#[test]
fn every_recording_holds_sampling() {
    assert_eq!(parse_tiers(&[]).unwrap(), vec![Tier::Sampling]);
    assert_eq!(parse_tiers(&["allocations".to_owned()]).unwrap(), vec![
        Tier::Sampling,
        Tier::Allocations
    ]);
}

/// Silently accepting the word would tell a caller they turned something on.
#[test]
fn asking_for_sampling_says_it_is_not_a_choice() {
    let error = parse_tiers(&["sampling".to_owned()])
        .unwrap_err()
        .to_string();

    assert!(
        error.starts_with("`capture` does not accept \"sampling\""),
        "unexpected error: {error}"
    );
}

#[test]
fn an_unknown_tier_is_rejected_by_name() {
    let error = parse_tiers(&["leaks".to_owned()]).unwrap_err().to_string();

    assert_eq!(
        error,
        "`capture` does not accept \"leaks\". The only value is \"allocations\"; sampling is \
         always recorded."
    );
}

#[test]
fn asking_for_allocations_twice_records_it_once() {
    assert_eq!(
        parse_tiers(&["allocations".to_owned(), "allocations".to_owned()]).unwrap(),
        vec![Tier::Sampling, Tier::Allocations]
    );
}

#[test]
fn a_recording_names_what_it_holds() {
    let sampling = recording("a", 1);
    assert_eq!(sampling.describe(), "sampling");
    assert!(sampling.holds(Tier::Sampling));
    assert!(!sampling.holds(Tier::Allocations));

    let both = Recording {
        tiers: vec![Tier::Sampling, Tier::Allocations],
        ..recording("b", 1)
    };
    assert_eq!(both.describe(), "sampling, allocations");
    assert!(both.holds(Tier::Allocations));
}

/// Killing the recorder leaves a bundle nothing can open, so the escalation
/// ladder every other stop in these tools uses must not apply here.
#[test]
fn stopping_the_recorder_only_ever_interrupts_it() {
    let signals = Recorded::dying_on(Signal::Int);

    let (outcome, _) = stop(4321, &signals, Duration::from_secs(2));

    assert_eq!(outcome, Stop::Finalized);
    assert_eq!(signals.sent(), vec![Signal::Int]);
}

#[test]
fn a_recorder_that_will_not_exit_is_reported_rather_than_killed() {
    let signals = Recorded::deaf();

    let (outcome, _) = stop(4321, &signals, Duration::from_millis(200));

    assert_eq!(outcome, Stop::Stuck);
    assert_eq!(signals.sent(), vec![Signal::Int]);
}

#[test]
fn a_recorder_already_gone_is_not_signalled() {
    let signals = Recorded::gone();

    let (outcome, _) = stop(4321, &signals, Duration::from_secs(2));

    assert_eq!(outcome, Stop::Absent);
    assert_eq!(signals.sent(), vec![]);
}

#[test]
fn recording_is_recognized_from_what_the_recorder_prints() {
    assert!(is_recording("Ctrl-C to stop the recording\n"));
    assert!(is_recording(
        "Starting recording with the Blank template and Time Profiler Instrument.\n"
    ));
    assert!(!is_recording(""));
    assert!(!is_recording("Starting run...\n"));
}

/// The exit status is useless here — a completed run carries the status of
/// what it recorded — so this string and a bundle on disk are the whole
/// signal.
#[test]
fn run_issues_are_read_out_of_the_recorders_own_words() {
    assert!(run_issues(
        "Recording completed.\nRun issues were detected. See the trace for details.\n"
    ));
    assert!(!run_issues("Recording completed.\n"));
}

/// A bracket whose recorder is alive is the one case a sweep must leave alone.
#[test]
fn an_open_bracket_is_pending_and_survives_a_sweep() {
    let (_workspace, dir) = temp();
    let open = recording("profile-1", std::process::id());
    on_disk(&dir, &open);

    assert_eq!(pending(&dir).map(|r| r.id), Some("profile-1".to_owned()));
    assert_eq!(sweep(&dir, &Recorded::gone()), Vec::<String>::new());
    assert!(open.bundle(&dir).exists());
}

/// The defect this replaced: keying on liveness made a recorder that failed on
/// its own invisible, so `stop` reported no open bracket and the sweep
/// destroyed the bundle along with the log line saying what went wrong.
/// A dead recorder is still a bracket with data in it.
#[test]
fn a_bracket_whose_recorder_died_is_still_pending() {
    let (_workspace, dir) = temp();
    let failed = recording("profile-1", DEAD_PID);
    on_disk(&dir, &failed);
    fs::write(
        failed.log(&dir),
        "Ctrl-C to stop the recording\nRun issues were detected (trace is still ready to be \
         viewed):\n* [Error] Allocations cannot handle a target type of 'All Processes'\n",
    )
    .unwrap();

    assert_eq!(pending(&dir).map(|r| r.id), Some("profile-1".to_owned()));
    assert_eq!(sweep(&dir, &Recorded::gone()), Vec::<String>::new());
    assert!(failed.bundle(&dir).exists());
}

/// A bracket nobody ever closed still has to be reclaimed, or its bundle sits
/// on disk forever.
/// Age is what decides that, since liveness no longer does.
#[test]
fn a_bracket_older_than_the_window_is_swept() {
    let (_workspace, dir) = temp();
    let stale = Recording {
        started_unix: super::unix_seconds() - PENDING_WINDOW.as_secs() - 1,
        ..recording("profile-1", std::process::id())
    };
    on_disk(&dir, &stale);

    assert_eq!(pending(&dir), None);
    assert_eq!(sweep(&dir, &Recorded::gone()), vec!["profile-1".to_owned()]);
    assert!(!stale.bundle(&dir).exists());
    assert!(!stale.log(&dir).exists());
    assert!(!stale.sidecar(&dir).exists());
}

/// A summary means the bracket already closed, whatever else is on disk.
#[test]
fn a_closed_bracket_is_not_pending() {
    let (_workspace, dir) = temp();
    let closed = recording("profile-1", std::process::id());
    on_disk(&dir, &closed);
    fs::write(closed.summary(&dir), "# profile-1\n").unwrap();

    assert_eq!(pending(&dir), None);
}

/// Summaries are the product and hold nothing the bundle held, so a sweep keeps
/// them.
#[test]
fn sweeping_keeps_the_summary() {
    let (_workspace, dir) = temp();
    let closed = Recording {
        scope: Scope::System,
        ..recording("profile-1", DEAD_PID)
    };
    on_disk(&dir, &closed);
    fs::write(closed.summary(&dir), "# profile-1\n").unwrap();

    sweep(&dir, &Recorded::gone());

    assert!(!closed.bundle(&dir).exists());
    assert_eq!(
        fs::read_to_string(closed.summary(&dir)).unwrap(),
        "# profile-1\n"
    );
}

/// The discriminator for retention is the same one that decides everything else
/// here.
/// An attach bundle holds this app's environment alone, so re-scoping it,
/// comparing it against another run, and recovering from a bad read are all
/// possible; a system-wide one holds the whole machine's and cannot be kept.
#[test]
fn a_closed_attach_recording_keeps_its_bundle_and_a_system_one_does_not() {
    let (_workspace, dir) = temp();

    let attached = recording("profile-1", DEAD_PID);
    on_disk(&dir, &attached);
    fs::write(attached.summary(&dir), "# profile-1\n").unwrap();

    let system = Recording {
        scope: Scope::System,
        ..recording("profile-2", DEAD_PID)
    };
    on_disk(&dir, &system);
    fs::write(system.summary(&dir), "# profile-2\n").unwrap();

    assert_eq!(sweep(&dir, &Recorded::gone()), vec!["profile-2".to_owned()]);

    assert!(attached.bundle(&dir).exists());
    assert!(attached.sidecar(&dir).exists());
    assert!(!system.bundle(&dir).exists());

    // The system recording's record outlives its bundle, so a report can still
    // say which app the surviving summary belongs to and why there is nothing to
    // re-read.
    assert!(system.sidecar(&dir).exists());
}

/// A read that failed used to leave the bracket looking open forever, which
/// blocked the next one.
/// The stop stamp is what closes it regardless.
#[test]
fn a_stopped_bracket_is_closed_even_with_no_summary_written() {
    let (_workspace, dir) = temp();
    let mut stopped = recording("profile-1", DEAD_PID);
    on_disk(&dir, &stopped);

    assert_eq!(pending(&dir).map(|r| r.id), Some("profile-1".to_owned()));

    stopped.close(None, &dir).unwrap();

    assert_eq!(pending(&dir), None);
    assert!(!stopped.summary(&dir).exists());
}

/// Retention has no meaning without something that answers "which app?", and
/// `debug_app_quit` removes the record that otherwise would.
#[test]
fn a_closed_recording_carries_what_it_was_recording() {
    let (_workspace, dir) = temp();
    let mut closed = recording("profile-1", DEAD_PID);
    on_disk(&dir, &closed);

    closed
        .close(
            Some(super::Target {
                pid: 31657,
                binary: "/staged/JP.app/Contents/MacOS/JP".into(),
                dsym: None,
                slide: Some(xct2cli::Slide::new(0x4000)),
                configuration: "Debug".to_owned(),
                uuid: None,
            }),
            &dir,
        )
        .unwrap();

    let raw = fs::read_to_string(closed.sidecar(&dir)).unwrap();
    let loaded: Recording = serde_json::from_str(&raw).unwrap();
    let target = loaded.target.unwrap();

    assert_eq!(target.pid, 31657);
    assert_eq!(target.binary, "/staged/JP.app/Contents/MacOS/JP");
    assert_eq!(target.slide, Some(xct2cli::Slide::new(0x4000)));
    assert_eq!(target.configuration, "Debug");
}

/// "48 hours" bounds nothing about size: a system-wide recording pulled in
/// around 450 symbol archives, and filling the disk is a demonstrated failure
/// here.
#[test]
fn the_byte_budget_evicts_the_oldest_first() {
    let (_workspace, dir) = temp();

    let oldest = Recording {
        started_unix: super::unix_seconds() - 300,
        ..recording("profile-old", DEAD_PID)
    };
    let newest = Recording {
        started_unix: super::unix_seconds(),
        ..recording("profile-new", DEAD_PID)
    };

    for held in [&oldest, &newest] {
        on_disk(&dir, held);
        fs::write(held.bundle(&dir).join("corespace/bulk"), vec![0_u8; 4096]).unwrap();
    }

    // Room for one of the two.
    let swept = enforce_limits(
        &dir,
        vec![
            Artifact::Bundle(newest.clone()),
            Artifact::Bundle(oldest.clone()),
        ],
        5_000,
    );

    assert_eq!(swept, vec!["profile-old".to_owned()]);
    assert!(!oldest.bundle(&dir).exists());
    assert!(newest.bundle(&dir).exists());
}

/// The per-step counts live in the app's own stream and nowhere else, so a
/// launch that truncated it would make cross-run comparison impossible whatever
/// happened to the bundles.
#[test]
fn the_apps_stream_is_archived_rather_than_truncated() {
    let (_workspace, dir) = temp();
    let stream = dir.join("state/trace.jsonl");
    fs::create_dir_all(stream.parent().unwrap()).unwrap();
    fs::write(&stream, "{\"timestamp\":\"x\"}\n").unwrap();

    let id = archive_stream(&dir, &stream).unwrap().unwrap();

    assert!(id.starts_with("trace-"));
    assert!(!stream.exists());
    assert_eq!(streams(&dir).len(), 1);
    assert_eq!(
        fs::read_to_string(&streams(&dir)[0]).unwrap(),
        "{\"timestamp\":\"x\"}\n"
    );
}

#[test]
fn archiving_an_empty_stream_leaves_nothing_behind() {
    let (_workspace, dir) = temp();
    let stream = dir.join("state/trace.jsonl");
    fs::create_dir_all(stream.parent().unwrap()).unwrap();
    fs::write(&stream, "").unwrap();

    assert_eq!(archive_stream(&dir, &stream).unwrap(), None);
    assert_eq!(streams(&dir), Vec::<camino::Utf8PathBuf>::new());
}

/// An archived stream is retained on the same terms as a bundle, so it does not
/// accumulate forever either.
#[test]
fn an_archived_stream_older_than_the_window_is_swept() {
    let (_workspace, dir) = temp();
    let stale_ms = (super::unix_seconds() - RETENTION_WINDOW.as_secs() - 60) * 1000;
    let path = profiles_dir(&dir).join(format!("trace-{stale_ms}.jsonl"));
    fs::create_dir_all(profiles_dir(&dir)).unwrap();
    fs::write(&path, "{}\n").unwrap();

    assert_eq!(sweep(&dir, &Recorded::gone()), vec![format!(
        "trace-{stale_ms}"
    )]);
    assert!(!path.exists());
}

/// A bracket that died between creating its bundle and writing its record
/// leaves a bundle nothing refers to.
/// It is still recorded environments.
#[test]
fn a_bundle_with_no_record_is_swept() {
    let (_workspace, dir) = temp();
    let stray = profiles_dir(&dir).join("profile-orphan.trace");
    fs::create_dir_all(&stray).unwrap();

    assert_eq!(sweep(&dir, &Recorded::gone()), vec![
        "profile-orphan".to_owned()
    ]);
    assert!(!stray.exists());
}

#[test]
fn sweeping_a_slot_that_never_recorded_finds_nothing() {
    let (_workspace, dir) = temp();

    assert_eq!(sweep(&dir, &Recorded::gone()), Vec::<String>::new());
    assert_eq!(pending(&dir), None);
}

/// The whole log is the recorder's progress chatter; the reason a recording
/// failed is the marker line and the bullets under it, and that is what a
/// report has room for.
#[test]
fn run_issue_lines_keep_the_reason_and_drop_the_chatter() {
    let said = "Starting recording with the Blank template and Time Profiler, Allocations \
                Instruments. Targeting All Processes.\nCtrl-C to stop the recording\nRun issues \
                were detected (trace is still ready to be viewed):\n* [Error] Allocations cannot \
                handle a target type of 'All Processes'\n\nRecording failed with errors. Saving \
                output file...\n";

    assert_eq!(
        run_issue_lines(said),
        "Run issues were detected (trace is still ready to be viewed):\n* [Error] Allocations \
         cannot handle a target type of 'All Processes'"
    );
    assert_eq!(run_issue_lines("Recording completed.\n"), "");
}

#[test]
fn a_recording_round_trips_through_its_sidecar() {
    let (_workspace, dir) = temp();
    let original = Recording {
        tiers: vec![Tier::Sampling, Tier::Allocations],
        scope: Scope::System,
        ..recording("profile-1", DEAD_PID)
    };
    original.store(&dir).unwrap();

    let raw = fs::read_to_string(original.sidecar(&dir)).unwrap();
    let loaded: Recording = serde_json::from_str(&raw).unwrap();

    assert_eq!(loaded.id, "profile-1");
    assert_eq!(loaded.tiers, vec![Tier::Sampling, Tier::Allocations]);
    assert_eq!(loaded.scope, Scope::System);
    assert!(loaded.scope.is_system());
}
