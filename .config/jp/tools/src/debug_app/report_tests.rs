use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use chrono::DateTime;
use jp_tool::Outcome;
use serde_json::{Value, json};

use super::{Request, View, duration_ms, instant, run};
use crate::debug_app::{
    capture::{self, Recording, Scope, Target, Tier, profiles_dir},
    marks::{self, Mark},
    session::{Session, Slot, state_dir},
};

/// `2020-01-01T00:00:00Z`, in milliseconds.
///
/// Fixed and comfortably in the past, so every window a report derives from the
/// current clock holds the whole fixture whenever the suite runs.
const BASE_MS: u64 = 1_577_836_800_000;

/// Where an earlier run sits, far enough back that the two windows cannot
/// overlap.
const EARLIER_MS: u64 = BASE_MS - 800_000;

/// A slot every test in this file shares, so paths are predictable.
fn dir_for(root: &Utf8Path) -> Utf8PathBuf {
    Session::dir(root, &Slot::fixed("test"))
}

/// What a report said, whichever way it went.
///
/// Every failure this tool has is a caller-correctable one, so it comes back as
/// an `Outcome::Error` rather than as a `Result::Err`.
fn content(outcome: Outcome) -> String {
    match outcome {
        Outcome::Success { content } => content,
        Outcome::Error { message, .. } => message,
        other @ Outcome::NeedsInput { .. } => panic!("unexpected outcome: {other:?}"),
    }
}

fn reported(root: &Utf8Path, request: &Request) -> String {
    content(run(root, &dir_for(root), request).unwrap())
}

/// `at_ms` as the app spells a timestamp.
fn stamp(at_ms: u64) -> String {
    let time = DateTime::from_timestamp_millis(at_ms.cast_signed()).unwrap();

    format!("{}000Z", time.format("%Y-%m-%dT%H:%M:%S%.3f"))
}

/// One interval the app timed.
fn interval(at_ms: u64, target: &str, name: &str, duration_ms: f64, footprint_mb: u64) -> Value {
    json!({
        "timestamp": stamp(at_ms),
        "level": "INFO",
        "target": target,
        "fields": {
            "message": name,
            "duration_ms": duration_ms,
            "footprint_mb": footprint_mb,
        },
    })
}

/// The same, nested inside the interval that caused it.
fn nested(at_ms: u64, target: &str, name: &str, duration_ms: f64, span: &str) -> Value {
    json!({
        "timestamp": stamp(at_ms),
        "level": "INFO",
        "target": target,
        "fields": { "message": name, "duration_ms": duration_ms },
        "spans": [{ "name": span }],
    })
}

/// One selection: an outer interval, one FFI call under it, and `bodies` view
/// bodies afterwards, all inside the step window opening at `at_ms`.
fn selection(at_ms: u64, select_ms: f64, bodies: usize, footprint_mb: u64) -> Vec<Value> {
    let mut out = vec![
        interval(
            at_ms + 100,
            "JP.App",
            "conversation.select",
            select_ms,
            footprint_mb,
        ),
        nested(
            at_ms + 120,
            "JP.FFI",
            "storage.read",
            8.0,
            "conversation.select",
        ),
    ];

    for index in 0..bodies {
        out.push(interval(
            at_ms + 200 + index as u64 * 10,
            "JP.App",
            "ConversationHistoryView.body",
            0.4,
            footprint_mb + 1,
        ));
    }

    out
}

fn mark(run: &str, step: usize, at_ms: u64, row: &str) -> Mark {
    Mark {
        run: run.to_owned(),
        step,
        label: format!("select {{\"identifier\":\"sidebar.row.{row}\"}}"),
        began_ms: at_ms,
        ended_ms: at_ms + 500,
    }
}

fn write_stream(path: &Utf8Path, lines: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        format!(
            "{}\n",
            lines
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        ),
    )
    .unwrap();
}

/// A slot holding one driven run of three selections, whose view-body count
/// doubles while its FFI calls stay flat.
fn driven(root: &Utf8Path) -> Utf8PathBuf {
    let dir = dir_for(root);

    let mut lines = selection(BASE_MS, 40.0, 2, 180);
    lines.extend(selection(BASE_MS + 1_000, 60.0, 4, 190));
    lines.extend(selection(BASE_MS + 2_000, 90.0, 8, 204));
    write_stream(&state_dir(&dir).join("trace.jsonl"), &lines);

    marks::append(&dir, &[
        mark("drive-1", 1, BASE_MS, "a"),
        mark("drive-1", 3, BASE_MS + 1_000, "b"),
        mark("drive-1", 5, BASE_MS + 2_000, "c"),
    ])
    .unwrap();

    dir
}

/// An earlier run of the same list, archived at the launch that replaced it,
/// where every selection cost two body evaluations.
fn archived_earlier_run(dir: &Utf8Path) {
    let mut lines = selection(EARLIER_MS, 40.0, 2, 150);
    lines.extend(selection(EARLIER_MS + 1_000, 40.0, 2, 150));
    lines.extend(selection(EARLIER_MS + 2_000, 40.0, 2, 150));
    write_stream(
        &profiles_dir(dir).join(format!("trace-{EARLIER_MS}.jsonl")),
        &lines,
    );

    marks::append(dir, &[
        mark("drive-0", 1, EARLIER_MS, "a"),
        mark("drive-0", 3, EARLIER_MS + 1_000, "b"),
        mark("drive-0", 5, EARLIER_MS + 2_000, "c"),
    ])
    .unwrap();
}

/// A recording covering `[from_ms, to_ms]`, with a bundle on disk.
fn recorded(dir: &Utf8Path, id: &str, tiers: Vec<Tier>, from_ms: u64, to_ms: Option<u64>) {
    let recording = Recording {
        id: id.to_owned(),
        tiers,
        scope: Scope::Attach(31657),
        recorder_pid: 4_000_000,
        started_unix: from_ms / 1000,
        stopped_unix: to_ms.map(|to_ms| to_ms / 1000),
        target: Some(Target {
            pid: 31657,
            binary: dir.join("JP.app/Contents/MacOS/JP"),
            dsym: None,
            slide: None,
            configuration: "Debug".to_owned(),
            uuid: None,
        }),
    };

    fs::create_dir_all(recording.bundle(dir)).unwrap();
    recording.store(dir).unwrap();
}

/// A bracket opened just now and still open.
///
/// Dated from the current clock rather than from the fixture, because a bracket
/// stops being pending once it is older than the window it is allowed to stay
/// open for — a recording dated 2020 reads as abandoned, not as open.
fn open_now(dir: &Utf8Path, id: &str) {
    let started = capture::unix_seconds();

    recorded(dir, id, vec![Tier::Sampling], started * 1000, None);
}

/// Close a bracket the way a stop does, keeping what it was recording.
fn close(dir: &Utf8Path, id: &str) {
    let mut recording = capture::recordings(dir)
        .into_iter()
        .find(|recording| recording.id == id)
        .unwrap();
    let target = recording.target.clone();

    recording.close(target, dir).unwrap();
}

#[test]
fn every_view_round_trips_through_its_name() {
    for view in [
        View::Timeline,
        View::Spans,
        View::Views,
        View::Hotspots,
        View::Callgraph,
        View::Allocations,
    ] {
        assert_eq!(View::parse(view.label()).unwrap(), view);
    }
}

#[test]
fn an_unknown_view_is_rejected_by_name() {
    let error = View::parse("flamegraph").unwrap_err().to_string();

    assert_eq!(
        error,
        "`view` accepts \"timeline\", \"spans\", \"views\", \"hotspots\", \"callgraph\" or \
         \"allocations\", not \"flamegraph\"."
    );
}

#[test]
fn a_relative_window_counts_back_from_now() {
    assert_eq!(duration_ms("30s"), Some(30_000));
    assert_eq!(duration_ms("5m"), Some(300_000));
    assert_eq!(duration_ms("2h"), Some(7_200_000));
    assert_eq!(duration_ms("30"), None);
    assert_eq!(duration_ms("s"), None);
    assert_eq!(instant("30s", "since", 100_000).unwrap(), 70_000);
}

#[test]
fn an_absolute_window_is_read_as_rfc_3339() {
    assert_eq!(
        instant("2020-01-01T00:00:00Z", "since", 0).unwrap(),
        BASE_MS
    );
}

#[test]
fn a_window_that_is_neither_form_names_both() {
    let error = instant("yesterday", "since", 0).unwrap_err().to_string();

    assert!(
        error.starts_with(
            "`since` accepts a duration back from now (`30s`, `5m`, `2h`) or an RFC 3339 timestamp"
        ),
        "unexpected error: {error}"
    );
}

#[test]
fn an_empty_request_names_nothing() {
    assert!(Request::default().is_empty());
    assert_eq!(Request::default().named(), Vec::<&str>::new());
}

#[test]
fn a_request_names_what_was_given() {
    let request = Request {
        view: Some("views".to_owned()),
        step: Some(3),
        ..Request::default()
    };

    assert!(!request.is_empty());
    assert_eq!(request.named(), vec!["view", "step"]);
}

/// The whole point of the default view: counts attributed to the step that
/// caused them, a reading of what the columns show while the pattern is clear,
/// and the calls that answer the next question.
#[test]
fn a_timeline_attributes_the_apps_intervals_to_the_steps_that_caused_them() {
    let workspace = camino_tempfile::tempdir().unwrap();
    driven(workspace.path());

    assert_eq!(
        reported(workspace.path(), &Request::default()),
        "`drive-1`: 3 steps, from the app's own intervals.\n\n| # | Step | Traced | View bodies | \
         FFI calls | Footprint |\n| -: | :--- | ---: | ---: | ---: | ---: |\n| 1 | select \
         {\"identifier\":\"sidebar.row.a\"} | 41 ms | 2 | 1 | 181 MB |\n| 3 | select \
         {\"identifier\":\"sidebar.row.b\"} | 62 ms | 4 | 1 | 191 MB |\n| 5 | select \
         {\"identifier\":\"sidebar.row.c\"} | 93 ms | 8 | 1 | 205 MB |\n\nView-body count grows \
         from 2 to 8 across these steps while FFI calls stay at 1. The cost is re-evaluation, not \
         loading.\n\nThe `View bodies` column counts the two view bodies the app instruments, not \
         the whole view tree.\n\n## Next\n\n- view=`views` step=`<n>` — which bodies ran under \
         one step, and how often\n- view=`spans` — every interval the app timed, by how often it \
         ran\n- `mode: \"start\"`, drive the operation, `mode: \"stop\"` — then `view: \
         \"hotspots\"` can name the code responsible\n"
    );
}

/// A selection's read runs on its own task, so the harness sees the sidebar
/// change and moves on while the transcript is still loading.
/// Attributing an interval to the step whose window holds its *end* therefore
/// files a slow selection under the following step — or under no step at all
/// when it outlives the run — and the table reports one step's cost against
/// another's name.
///
/// The numbers here are the ones that exposed it: step 4's selection took
/// 48.9ms and ended 9ms after step 4's window closed; step 5's took 85.0ms and
/// ended 44ms after the whole run finished.
#[test]
fn a_selection_outliving_its_step_is_still_attributed_to_it() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let dir = dir_for(root);

    write_stream(&state_dir(&dir).join("trace.jsonl"), &[
        // Begins 85ms into step 4's window, ends 9ms after it closed — inside
        // step 5's.
        interval(BASE_MS + 3_208, "JP.App", "conversation.select", 48.932, 42),
        // Begins 405ms into step 5's window, ends 44ms past the end of the run.
        interval(
            BASE_MS + 3_688,
            "JP.App",
            "conversation.select",
            84.992,
            101,
        ),
    ]);

    marks::append(&dir, &[
        Mark {
            run: "drive-late".to_owned(),
            step: 4,
            label: "select 4".to_owned(),
            began_ms: BASE_MS + 3_075,
            ended_ms: BASE_MS + 3_199,
        },
        Mark {
            run: "drive-late".to_owned(),
            step: 5,
            label: "select 5".to_owned(),
            began_ms: BASE_MS + 3_199,
            ended_ms: BASE_MS + 3_644,
        },
    ])
    .unwrap();

    let report = reported(root, &Request::default());

    assert!(
        report.contains(
            "| 4 | select 4 | 49 ms | 0 | 0 | 42 MB |\n| 5 | select 5 | 85 ms | 0 | 0 | 101 MB |\n"
        ),
        "unexpected report: {report}"
    );
}

/// `debug_app_quit` removes the session record, and reading a run afterwards is
/// the ordinary case rather than an edge.
#[test]
fn a_report_after_quit_works_from_the_slot_directory_alone() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let dir = driven(workspace.path());
    recorded(
        &dir,
        "profile-1",
        vec![Tier::Sampling],
        BASE_MS,
        Some(BASE_MS + 3_000),
    );

    assert!(!Session::path(&dir).exists());

    let report = reported(workspace.path(), &Request::default());

    assert!(report.starts_with("`drive-1`: 3 steps"), "{report}");
    assert!(
        report.contains("| `profile-1` | attach | sampling | closed | kept |"),
        "{report}"
    );
}

/// The two tiers answer at different moments, and the difference has to be
/// visible rather than inferred: a bracket that is still recording has no
/// finalized bundle, because `xctrace` writes one out on its way to exiting.
#[test]
fn an_open_bracket_and_a_closed_one_read_differently() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let dir = driven(root);
    open_now(&dir, "profile-open");

    let while_open = reported(root, &Request::default());
    assert!(
        while_open.contains("| `profile-open` | attach | sampling | open | kept |"),
        "unexpected report: {while_open}"
    );

    let hotspots = Request {
        view: Some("hotspots".to_owned()),
        ..Request::default()
    };
    let refusal = "`view: \"hotspots\"` reads a finalized `.trace`, and the only recording in \
                   this slot (`profile-open`) is still open. Close it with `mode: \"stop\"` \
                   first. Until then, `view: \"timeline\"`, `view: \"spans\"` and `view: \
                   \"views\"` answer from the app's own intervals and work while it records.";
    assert_eq!(reported(root, &hotspots), refusal);

    // Closing it is the only change.
    close(&dir, "profile-open");

    let once_closed = reported(root, &Request::default());
    assert!(
        once_closed.contains("| `profile-open` | attach | sampling | closed | kept |"),
        "unexpected report: {once_closed}"
    );

    // Past the open-bracket gate, so the read reaches the bundle itself. What it
    // finds in there is not assertable without `xctrace`; the live end-to-end run
    // is what covers that.
    let read = reported(root, &hotspots);
    assert!(!read.contains("is still open"), "unexpected report: {read}");
    assert_ne!(read, refusal);
}

/// The gate order, pinned from the other side: a closed recording is checked
/// for a bundle rather than for being open, and the two say different things.
#[test]
fn a_closed_recording_whose_bundle_was_reclaimed_says_so() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let dir = driven(root);
    recorded(
        &dir,
        "profile-1",
        vec![Tier::Sampling],
        BASE_MS,
        Some(BASE_MS + 3_000),
    );
    fs::remove_dir_all(dir.join("profiles/profile-1.trace")).unwrap();

    assert_eq!(
        reported(root, &Request {
            view: Some("hotspots".to_owned()),
            recording: Some("profile-1".to_owned()),
            ..Request::default()
        }),
        "`profile-1` no longer has a bundle. A retained bundle is bounded by age and by a byte \
         budget, and this one has been reclaimed; its summary is kept at `profile-1.md` under \
         this slot's `profiles/`. Record a new bracket for `view: \"hotspots\"`."
    );
}

/// Asking about memory is not refused for want of an instrument.
/// The footprint is the answer, the app samples it on every run, and what the
/// recording lacks is said in the body rather than instead of it.
///
/// Reaching the bundle at all is what the tier gates, and there is nothing to
/// gate here: the Allocations instrument's data is not exportable, so a
/// recording that holds it can say no more about call sites than one that does
/// not.
#[test]
fn allocations_reports_the_footprint_and_says_what_the_recording_lacks() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let dir = driven(root);
    recorded(
        &dir,
        "profile-1",
        vec![Tier::Sampling],
        BASE_MS,
        Some(BASE_MS + 3_000),
    );

    let report = reported(root, &Request {
        view: Some("allocations".to_owned()),
        ..Request::default()
    });

    assert!(
        report.contains(
            "`profile-1` recorded sampling, so it holds no allocation stacks. The footprint above \
             needs none"
        ),
        "unexpected report: {report}"
    );
}

/// Counts are comparable between runs and milliseconds are not, which is the
/// whole reason this view leads on them.
/// The earlier run is only reachable because its stream was archived rather
/// than truncated.
#[test]
fn comparing_two_recordings_shows_count_deltas() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let dir = driven(root);
    archived_earlier_run(&dir);

    recorded(
        &dir,
        "profile-earlier",
        vec![Tier::Sampling],
        EARLIER_MS,
        Some(EARLIER_MS + 3_000),
    );
    recorded(
        &dir,
        "profile-now",
        vec![Tier::Sampling],
        BASE_MS,
        Some(BASE_MS + 3_000),
    );

    let report = reported(root, &Request {
        recording: Some("profile-now".to_owned()),
        against: Some("profile-earlier".to_owned()),
        ..Request::default()
    });

    assert!(
        report.contains(
            "| # | Step | View bodies | Δ | FFI calls | Δ |\n| -: | :--- | ---: | ---: | ---: | \
             ---: |\n| 1 | select {\"identifier\":\"sidebar.row.a\"} | 2 | — | 1 | — |\n| 3 | \
             select {\"identifier\":\"sidebar.row.b\"} | 4 | +2 | 1 | — |\n| 5 | select \
             {\"identifier\":\"sidebar.row.c\"} | 8 | +6 | 1 | — |\n"
        ),
        "unexpected report: {report}"
    );
}

/// A report is pasted into issues, and a path from the machine that produced it
/// names somebody's filesystem.
/// Both the report and the refusal go through the shortening, since a refusal
/// is the more likely of the two to quote a path.
#[test]
fn no_absolute_path_reaches_the_output_on_either_path() {
    let empty = camino_tempfile::tempdir().unwrap();
    let failed = reported(empty.path(), &Request::default());
    assert!(
        !failed.contains(empty.path().as_str()) && !failed.contains(" /"),
        "the failure names a filesystem: {failed}"
    );

    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let dir = driven(root);
    archived_earlier_run(&dir);
    recorded(
        &dir,
        "profile-1",
        vec![Tier::Sampling],
        BASE_MS,
        Some(BASE_MS + 3_000),
    );

    for request in [
        Request::default(),
        Request {
            view: Some("spans".to_owned()),
            ..Request::default()
        },
        Request {
            view: Some("views".to_owned()),
            step: Some(5),
            ..Request::default()
        },
        Request {
            // A window holding no driven steps, which is the branch that names
            // where the step record lives.
            since: Some("1h".to_owned()),
            ..Request::default()
        },
        Request {
            // A slot with no such recording, which is the branch that lists them.
            recording: Some("profile-absent".to_owned()),
            ..Request::default()
        },
    ] {
        let report = reported(root, &request);

        assert!(
            !report.contains(root.as_str()),
            "the report names {root}: {report}"
        );
        assert!(
            !report.contains(" /") && !report.contains("`/"),
            "the report holds an absolute path: {report}"
        );
    }
}

#[test]
fn scoping_to_one_step_narrows_the_table_to_it() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    driven(root);

    let report = reported(root, &Request {
        step: Some(3),
        ..Request::default()
    });

    assert!(
        report.starts_with(
            "`drive-1`: One step, from the app's own intervals.\n\n| # | Step | Traced | View \
             bodies | FFI calls | Footprint |\n| -: | :--- | ---: | ---: | ---: | ---: |\n| 3 | \
             select {\"identifier\":\"sidebar.row.b\"} | 62 ms | 4 | 1 | 191 MB |\n"
        ),
        "unexpected report: {report}"
    );
    assert!(!report.contains("sidebar.row.a"), "{report}");
}

#[test]
fn a_step_that_names_nothing_lists_what_there_is() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    driven(root);

    assert_eq!(
        reported(root, &Request {
            step: Some(2),
            ..Request::default()
        }),
        "`step: 2` names nothing in `drive-1`, which has 3: 1, 3, 5."
    );
}

/// The app's launch, and anything done by hand, has no steps around it by
/// construction.
/// A report says so and names the view that still answers.
#[test]
fn a_window_with_no_driven_steps_names_the_view_that_still_answers() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    write_stream(&state_dir(&dir_for(root)).join("trace.jsonl"), &[interval(
        BASE_MS,
        "JP.App",
        "app.launch",
        900.0,
        120,
    )]);

    let report = reported(root, &Request::default());

    assert!(
        report.starts_with(
            "No driven steps fall in this window, so there is nothing to attribute per step. The \
             window holds 1 interval: 900 ms traced, 0 view bodies, 0 FFI calls."
        ),
        "unexpected report: {report}"
    );
    assert!(
        report.contains("Ask `view: \"spans\"` for what ran instead."),
        "{report}"
    );
}

#[test]
fn a_slot_the_app_never_ran_in_says_what_is_missing() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let report = reported(workspace.path(), &Request::default());

    assert!(
        report.starts_with("No traced intervals in tmp/debug-app/test/state."),
        "unexpected report: {report}"
    );
}

/// Silently dropping an argument leaves a caller believing they scoped
/// something.
#[test]
fn an_argument_that_does_not_apply_to_a_view_is_refused() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    driven(root);

    assert_eq!(
        reported(root, &Request {
            view: Some("hotspots".to_owned()),
            step: Some(1),
            ..Request::default()
        }),
        "`step` scopes the app's own intervals, which `view: \"hotspots\"` does not read. A \
         bundle holds samples with no notion of which step caused them. Use `view: \"timeline\"` \
         or `view: \"views\"` for per-step counts."
    );

    assert_eq!(
        reported(root, &Request {
            view: Some("callgraph".to_owned()),
            against: Some("profile-1".to_owned()),
            ..Request::default()
        }),
        "`against` compares counts, and `view: \"callgraph\"` reports sample counts — which are \
         time, and so noisy between runs. Comparing two of them chases ghosts. Compare `view: \
         \"timeline\"`, `view: \"spans\"` or `view: \"views\"` instead, which count work the app \
         did rather than moments a sampler caught it."
    );

    assert_eq!(
        reported(root, &Request {
            view: Some("spans".to_owned()),
            function: Some("deserialize".to_owned()),
            ..Request::default()
        }),
        "`function` names a symbol in the app's binary, which `view: \"spans\"` does not read. \
         Use `span` to narrow to an interval the app timed, or `view: \"hotspots\"` to narrow to \
         a symbol."
    );

    assert_eq!(
        reported(root, &Request {
            view: Some("views".to_owned()),
            top: Some(5),
            ..Request::default()
        }),
        "`top` bounds a bundle-backed table, and `view: \"views\"` shows every interval it found. \
         Narrow it with `span`, `step`, or a time window instead."
    );
}
