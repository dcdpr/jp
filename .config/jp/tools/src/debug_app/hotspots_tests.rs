use camino::Utf8Path;
use xct2cli::{
    RuntimePc,
    analysis::{Hotspot, SlideMode},
};

use super::{Extract, render, slide_mode};
use crate::{
    debug_app::capture::{Recording, Scope, Target, Tier},
    util::paths::{Shortening, shortenings_from},
};

/// A wrong load address does not fail, it lies: frames come back as bare
/// addresses, or as whatever symbol happens to sit at that offset, and the
/// table looks plausible either way.
/// So a recording's own reported address is what every read of its bundle uses,
/// and only a build that reports none falls back to recovering one from the
/// trace.
#[test]
fn a_reported_load_address_is_used_rather_than_recovered() {
    let reported = Target {
        slide: Some(xct2cli::Slide::new(0x4000)),
        ..target()
    };

    assert!(matches!(
        slide_mode(&reported),
        SlideMode::Manual(slide) if slide == xct2cli::Slide::new(0x4000)
    ));

    // Only a build that reports nothing falls back, and only that case can be
    // wrong about where the images were mapped.
    assert!(matches!(slide_mode(&target()), SlideMode::Auto));
}

/// A machine whose layout matches the paths the fixtures use.
fn shortenings() -> Vec<Shortening> {
    shortenings_from(
        Utf8Path::new("/Users/jean/Projects/jp"),
        Some("/Users/jean"),
        None,
        None,
    )
}

fn recording(tiers: Vec<Tier>, scope: Scope) -> Recording {
    Recording {
        id: "profile-1785748475000".to_owned(),
        tiers,
        scope,
        recorder_pid: 4321,
        started_unix: 1_785_748_475,
        stopped_unix: Some(1_785_748_480),
        target: Some(target()),
    }
}

fn target() -> Target {
    Target {
        pid: 31657,
        binary: "/derived/JP.app/Contents/MacOS/JP".into(),
        dsym: None,
        slide: None,
        configuration: "Debug".to_owned(),
        uuid: None,
    }
}

/// A frame the app's own dSYM could name.
fn named(samples: u64, function: &str, line: u32) -> Hotspot {
    Hotspot {
        pc: RuntimePc::new(0x1_0000_4a20),
        samples,
        fmt: None,
        function: Some(function.to_owned()),
        file: Some(
            "/Users/jean/Projects/jp/apps/macos/Sources/ConversationHistoryView.swift".to_owned(),
        ),
        line: Some(line),
    }
}

/// A frame in the dyld shared cache, which a trace carries no symbols for.
fn unnamed(samples: u64) -> Hotspot {
    Hotspot {
        pc: RuntimePc::new(0x1_9f1c_e3cc),
        samples,
        fmt: None,
        function: None,
        file: None,
        line: None,
    }
}

fn extract(top: Vec<Hotspot>, total_samples: u64) -> Extract {
    let mut hotspots = xct2cli::analysis::HotspotReport::empty(10_000_000);
    hotspots.total_samples = total_samples;
    hotspots.top_pcs = top;

    Extract {
        hotspots: Some(hotspots),
        duration: Some("37.087810".to_owned()),
        end_reason: Some("User pressed Stop".to_owned()),
        tables: vec!["time-sample".to_owned()],
    }
}

#[test]
fn renders_the_run_and_its_named_frames() {
    let summary = render(
        &recording(vec![Tier::Sampling], Scope::Attach(31657)),
        &extract(
            vec![
                named(200, "JP.ConversationHistoryView.body.getter", 88),
                named(50, "JP.WorkspaceModel.load()", 141),
            ],
            400,
        ),
        Some(&target()),
        &shortenings(),
    );

    assert_eq!(
        summary,
        "# `profile-1785748475000`\n\n- recorded: sampling\n- scope: the app alone\n- app: pid \
         31657, `Debug`\n- recording: 37.087810s\n- ended: User pressed Stop\n- tables: \
         time-sample\n\n## Time profile\n\n400 samples landed in the app.\n\n| samples | share | \
         function | site |\n| ---: | ---: | --- | --- |\n| 200 | 50.0% | \
         `JP.ConversationHistoryView.body.getter` | \
         apps/macos/Sources/ConversationHistoryView.swift:88 |\n| 50 | 12.5% | \
         `JP.WorkspaceModel.load()` | apps/macos/Sources/ConversationHistoryView.swift:141 \
         |\n\nThe `.trace` bundle is kept, so `debug_app_profile` with `mode: \"report\"` can ask \
         it further questions.\n"
    );
}

/// The defect this selection exists for: taking the busiest counters without
/// regard to whether they resolved produced a table of 29 bare addresses out of
/// 30, because an app's on-CPU time is mostly inside the dyld shared cache.
#[test]
fn keeps_the_named_frames_and_accounts_for_the_rest() {
    let mut top = vec![unnamed(23), unnamed(23), unnamed(17)];
    top.push(named(3, "JP.WorkspaceModel.load()", 141));

    let summary = render(
        &recording(vec![Tier::Sampling], Scope::Attach(31657)),
        &extract(top, 808),
        Some(&target()),
        &shortenings(),
    );

    assert!(
        summary.contains("| 3 | 0.4% | `JP.WorkspaceModel.load()` |"),
        "unexpected summary: {summary}"
    );
    assert!(
        !summary.contains("0x"),
        "a bare address reached the table: {summary}"
    );
    assert!(
        summary.contains(
            "3 of the 4 busiest program counters had no symbol in the app's binary. That is \
             system code in the dyld shared cache, which a trace carries no symbols for."
        ),
        "unexpected summary: {summary}"
    );
}

/// Every frame unresolved means either an idle bracket or a wrong slide, and a
/// silent empty table looks like neither.
#[test]
fn says_so_when_nothing_resolved_at_all() {
    let summary = render(
        &recording(vec![Tier::Sampling], Scope::Attach(31657)),
        &extract(vec![unnamed(23), unnamed(17)], 808),
        Some(&target()),
        &shortenings(),
    );

    assert!(
        summary.contains("None of the 2 busiest program counters resolved to a symbol"),
        "unexpected summary: {summary}"
    );
    assert!(summary.contains("the slide is wrong"), "{summary}");
    assert!(!summary.contains("| samples |"), "{summary}");
}

#[test]
fn says_why_an_empty_profile_is_empty() {
    let summary = render(
        &recording(vec![Tier::Sampling], Scope::Attach(31657)),
        &extract(vec![], 0),
        Some(&target()),
        &shortenings(),
    );

    assert!(
        summary.contains("No samples landed in the app."),
        "unexpected summary: {summary}"
    );
}

/// A bracket opened before any launch, into which nothing was ever launched.
#[test]
fn reports_a_system_recording_with_no_app() {
    let summary = render(
        &recording(vec![Tier::Sampling], Scope::System),
        &Extract {
            hotspots: None,
            duration: Some("12.0".to_owned()),
            end_reason: None,
            tables: Vec::new(),
        },
        None,
        &shortenings(),
    );

    assert!(
        summary.contains("- scope: every process on the machine"),
        "{summary}"
    );
    assert!(
        summary.contains("- app: none was launched into this recording"),
        "{summary}"
    );
    assert!(
        summary.contains("no app was launched into it"),
        "unexpected summary: {summary}"
    );
}

/// Numbers taken under `MallocStackLogging` are not comparable with numbers
/// taken without it, and nothing in the table itself says so.
#[test]
fn an_allocations_recording_warns_that_its_timings_are_distorted() {
    let summary = render(
        &recording(
            vec![Tier::Sampling, Tier::Allocations],
            Scope::Attach(31657),
        ),
        &extract(vec![named(1, "JP.main", 1)], 1),
        Some(&target()),
        &shortenings(),
    );

    assert!(
        summary.contains("- recorded: sampling, allocations"),
        "{summary}"
    );
    assert!(summary.contains("## Allocations"), "{summary}");
    assert!(
        summary.contains("costs 2x to 10x and not evenly"),
        "unexpected summary: {summary}"
    );
}

#[test]
fn a_pipe_in_a_symbol_does_not_break_the_table() {
    let summary = render(
        &recording(vec![Tier::Sampling], Scope::Attach(31657)),
        &extract(vec![named(1, "closure #1 (A|B) in JP.main", 1)], 1),
        Some(&target()),
        &shortenings(),
    );

    assert!(
        summary.contains(r"`closure #1 (A\|B) in JP.main`"),
        "unexpected summary: {summary}"
    );
}
