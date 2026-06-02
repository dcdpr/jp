use std::time::Duration;

use super::*;
use crate::debug_jp::util::launch::{LaunchResult, MockLauncher, Termination};

fn spec(workspace: &Utf8Path) -> LaunchSpec {
    LaunchSpec {
        binary: "/does/not/run".into(),
        args: vec!["c".to_owned(), "fork".to_owned()],
        working_dir: workspace.to_owned(),
        env: vec![],
    }
}

#[test]
fn renders_report_from_dhat_json() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();

    // Pre-stage the dhat JSON jp would have flushed under the sandbox working
    // dir (here the same temp dir), with one program point whose leaf is a jp
    // frame so it survives aggregation into the report.
    let dhat_dir = root.join("tmp/profiling");
    std::fs::create_dir_all(&dhat_dir).unwrap();
    std::fs::write(
        dhat_dir.join("heap-fixture.json"),
        r#"{"te":5000000,"tu":"instrs","pps":[{"tb":4096,"tbk":10,"gb":1024,"gbk":3,"eb":0,"ebk":0,"fs":[0,1]}],"ftbl":["0x1000: jp_config::PartialAppConfig::clone","0x1004: jp_conversation::Stream::extend"]}"#,
    )
    .unwrap();

    let launcher = MockLauncher::returning(LaunchResult {
        exit_code: Some(0),
        stdout: String::new(),
        stderr: String::new(),
        wall_duration: Duration::from_secs(1),
        termination: Termination::Exited,
    });

    let outcome = execute(root, &spec(root), &launcher, Timeouts::DEFAULT).unwrap();

    let Outcome::Success { content } = outcome else {
        panic!("expected a success outcome");
    };
    assert!(content.contains("heap (dhat)"), "got: {content}");
    // The staged program point's leaf frame made it through parse + render.
    assert!(
        content.contains("jp_config::PartialAppConfig::clone"),
        "got: {content}"
    );
    // A natural exit carries no shutdown-warning banner.
    assert!(!content.contains("[!WARNING]"));
}

#[test]
fn force_killed_without_heap_json_reports_note() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();

    // No dhat JSON is staged under the working dir, so the lookup fails; the
    // force-kill note must be folded into the resulting error.
    let launcher = MockLauncher::returning(LaunchResult {
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        wall_duration: Duration::from_secs(1),
        termination: Termination::Forced,
    });

    let error = execute(root, &spec(root), &launcher, Timeouts::DEFAULT)
        .unwrap_err()
        .to_string();

    assert!(error.contains("force-killed"), "got: {error}");
}
