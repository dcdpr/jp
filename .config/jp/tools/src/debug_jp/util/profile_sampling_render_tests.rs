use std::time::Duration;

use super::*;
use crate::debug_jp::util::{
    launch::Termination,
    profile_sampling_parse::{Frame, Thread},
};

fn fixture_threads() -> Vec<Thread> {
    vec![Thread {
        header: "Thread_42  com.apple.main-thread".into(),
        frames: vec![
            Frame {
                depth: 0,
                samples: 100,
                symbol: "jp_cli::run".into(),
            },
            Frame {
                depth: 1,
                samples: 100,
                symbol: "jp_cli::run_inner".into(),
            },
            Frame {
                depth: 2,
                samples: 80,
                symbol: "ConversationStream::extend".into(),
            },
            Frame {
                depth: 3,
                samples: 60,
                symbol: "PartialAppConfig::clone".into(),
            },
        ],
    }]
}

fn fixture_launch() -> LaunchResult {
    LaunchResult {
        exit_code: Some(0),
        stdout: String::new(),
        stderr: String::new(),
        wall_duration: Duration::from_millis(1234),
        termination: Termination::Exited,
    }
}

#[test]
fn render_includes_headline() {
    let report = render(
        &fixture_threads(),
        &fixture_launch(),
        &["c".into(), "fork".into()],
        "tmp/profiling/sample-test.txt",
    );
    assert!(report.contains("# jp profile · sampling"));
    assert!(report.contains("`jp c fork`"));
    assert!(report.contains("**Wall clock:** 1.23 s"));
    assert!(report.contains("**Status:** success"));
}

#[test]
fn render_includes_hot_leaves_table() {
    let report = render(
        &fixture_threads(),
        &fixture_launch(),
        &["c".into()],
        "x.txt",
    );
    assert!(report.contains("## Hot leaves"));
    assert!(report.contains("| 100 | `jp_cli::run` |"));
    assert!(report.contains("| 60 | `PartialAppConfig::clone` |"));
}

#[test]
fn render_reports_self_time_not_inclusive_totals() {
    // The fixture is a straight chain 100 -> 100 -> 80 -> 60, so self time is
    // 0, 20, 20, 60. A self-time table must surface the leaf, not the root that
    // merely contains it.
    let report = render(
        &fixture_threads(),
        &fixture_launch(),
        &["c".into()],
        "x.txt",
    );

    let start = report.find("## Hot code").expect("self-time section");
    let end = report[start..]
        .find("## Hot leaves")
        .map_or(report.len(), |off| start + off);
    let section = &report[start..end];

    assert!(
        section.contains("| 60 | 60.0% | `PartialAppConfig::clone` |"),
        "leaf should dominate self time; section was:\n{section}"
    );
    assert!(
        !section.contains("`jp_cli::run`"),
        "a frame that only calls others has no self time; section was:\n{section}"
    );
}

#[test]
fn render_hot_code_spans_worker_threads_and_excludes_parked_frames() {
    // A parked main thread plus a busy worker: the report must attribute the
    // work to the worker rather than showing only the latch wait.
    let threads = vec![
        Thread {
            header: "Thread_1  com.apple.main-thread".into(),
            frames: vec![
                Frame {
                    depth: 0,
                    samples: 90,
                    symbol: "jp_cli::run".into(),
                },
                Frame {
                    depth: 1,
                    samples: 90,
                    symbol: "_pthread_cond_wait".into(),
                },
            ],
        },
        Thread {
            header: "Thread_2".into(),
            frames: vec![
                Frame {
                    depth: 0,
                    samples: 90,
                    symbol: "rayon worker".into(),
                },
                Frame {
                    depth: 1,
                    samples: 70,
                    symbol: "serde_json::from_str".into(),
                },
            ],
        },
    ];
    let report = render(&threads, &fixture_launch(), &["c".into()], "x.txt");

    assert!(
        report.contains("`serde_json::from_str`"),
        "worker-thread work must appear:\n{report}"
    );
    assert!(
        report.contains("Parked/idle: 90"),
        "parked samples must be reported, not silently dropped:\n{report}"
    );
}

#[test]
fn render_includes_top_stacks() {
    let report = render(
        &fixture_threads(),
        &fixture_launch(),
        &["c".into()],
        "x.txt",
    );
    assert!(report.contains("## Hot stacks"));
    // The deepest, lowest-sample anchor still gets rendered with its ancestry.
    assert!(report.contains("PartialAppConfig::clone"));
}

#[test]
fn render_reports_failed_run() {
    let mut launch = fixture_launch();
    launch.exit_code = Some(2);
    launch.stderr = "Error: workspace not found\n".into();
    let report = render(&fixture_threads(), &launch, &["broken".into()], "x.txt");
    assert!(report.contains("**Status:** exit 2"));
    assert!(report.contains("workspace not found"));
}

#[test]
fn render_handles_empty_thread_list() {
    let report = render(&[], &fixture_launch(), &["c".into()], "x.txt");
    assert!(report.contains("No threads sampled"));
}

/// Regression: when the call tree branches, the ancestry rendered for an anchor
/// must follow the depth chain (parent = first preceding frame at depth-1), not
/// just take the previous N entries in document order.
///
/// Tree:
///
/// ```text
/// A (depth 0, 100)
/// ├─ B (depth 1, 60)
/// │  └─ C (depth 2, 40)
/// └─ D (depth 1, 30)
///    └─ E (depth 2, 30)
/// ```
///
/// Preorder: A, B, C, D, E. The ancestry for anchor E must be [A, D, E] — the
/// previous design would have returned [B, C, D, E].
#[test]
fn render_ancestry_follows_depth_chain_across_branches() {
    let threads = vec![Thread {
        header: "Thread_test".into(),
        frames: vec![
            Frame {
                depth: 0,
                samples: 100,
                symbol: "A".into(),
            },
            Frame {
                depth: 1,
                samples: 60,
                symbol: "B".into(),
            },
            Frame {
                depth: 2,
                samples: 40,
                symbol: "C".into(),
            },
            Frame {
                depth: 1,
                samples: 30,
                symbol: "D".into(),
            },
            Frame {
                depth: 2,
                samples: 30,
                symbol: "E".into(),
            },
        ],
    }];
    let report = render(&threads, &fixture_launch(), &["x".into()], "x.txt");

    // Locate the section for the E anchor (`30 samples @ depth 2`) and verify
    // the ancestry chain renders correctly.
    let section_start = report
        .find("30 samples @ depth 2")
        .expect("E section header should exist");
    let section_end = report[section_start..]
        .find("### ")
        .map_or(report.len(), |off| section_start + off);
    let section = &report[section_start..section_end];

    assert!(
        section.contains('A'),
        "E's ancestry should include A; section was:\n{section}"
    );
    assert!(
        section.contains('D'),
        "E's ancestry should include D; section was:\n{section}"
    );
    // B is at the same depth as D but is D's sibling, not an ancestor of E.
    // C is E's sibling under B, not an ancestor of E.
    assert!(
        !section.contains("  B"),
        "E's ancestry must NOT include B; section was:\n{section}"
    );
    assert!(
        !section.contains("  C"),
        "E's ancestry must NOT include C; section was:\n{section}"
    );
}
