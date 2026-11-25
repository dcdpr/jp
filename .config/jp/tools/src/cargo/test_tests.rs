use camino_tempfile::tempdir;
use jp_process::{ExitCode, MockProcessRunner};
use jp_tool::{Action, Context};
use pretty_assertions::assert_eq;

use super::*;

/// `cargo metadata` for a workspace holding the browser crate `a-ui`.
const BROWSER_METADATA: &str =
    r#"{ "packages": [{ "name": "a-ui", "metadata": { "jp": { "wasm": true } } }] }"#;

const BROWSER_TEST: &[&str] = &[
    "test",
    "--target",
    "wasm32-unknown-unknown",
    "--test",
    "*",
    "--package=a-ui",
];

#[test]
fn passing_browser_tests_are_counted() {
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success(BROWSER_METADATA)
        .expect("cargo")
        .args(BROWSER_TEST)
        .returns_success(
            "running 3 tests\ntest result: ok. 3 passed; 0 failed; 0 ignored; 0 filtered out\n",
        );

    let note = wasm_test(
        Utf8Path::new("/work"),
        "-W warnings",
        None,
        &[],
        None,
        None,
        &runner,
    );

    assert_eq!(
        note,
        Some(BrowserRun {
            note: "Browser tests: 3 passed, in a-ui.".to_owned(),
            ran: 3,
        })
    );
    let call = &runner.calls()[1];
    assert!(
        call.env.iter().any(
            |(key, value)| key == "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER"
                && value == "wasm-bindgen-test-runner"
        ),
        "the browser runs them: {:?}",
        call.env
    );
    assert!(
        !call.env.iter().any(|(key, _)| key == "CHROMEDRIVER"),
        "without a chromedriver, the runner picks the browser: {:?}",
        call.env
    );
}

#[test]
fn a_chromedriver_found_runs_the_browser_tests_in_chrome() {
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success(BROWSER_METADATA)
        .expect("cargo")
        .returns_success("test result: ok. 1 passed; 0 failed; 0 ignored\n");

    wasm_test(
        Utf8Path::new("/work"),
        "-W warnings",
        None,
        &[],
        None,
        Some("/opt/homebrew/bin/chromedriver"),
        &runner,
    );

    let call = &runner.calls()[1];
    assert!(
        call.env
            .iter()
            .any(|(key, value)| key == "CHROMEDRIVER" && value == "/opt/homebrew/bin/chromedriver"),
        "{:?}",
        call.env
    );
}

/// Chrome is preferred only while nothing names a browser: a `*DRIVER`
/// variable, local or remote, is left to pick it.
#[test]
fn chrome_is_used_unless_a_driver_variable_names_a_browser() {
    let found = || Some("/usr/local/bin/chromedriver".to_owned());

    assert_eq!(
        chromedriver_for(|_| false, found()).as_deref(),
        Some("/usr/local/bin/chromedriver")
    );
    assert_eq!(chromedriver_for(|_| false, None), None, "none on the PATH");
    assert_eq!(
        chromedriver_for(|name| name == "SAFARIDRIVER", found()),
        None
    );
    assert_eq!(
        chromedriver_for(|name| name == "GECKODRIVER_REMOTE", found()),
        None
    );
}

#[test]
fn failing_browser_tests_say_what_running_them_needs() {
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success(BROWSER_METADATA)
        .expect("cargo")
        .returns(ProcessOutput {
            stdout: "test result: FAILED. 1 passed; 2 failed; 0 ignored\n".to_owned(),
            stderr: "error: test failed, to rerun pass `--test highlight`\n".to_owned(),
            status: ExitCode::from_code(101),
        });

    let note = wasm_test(
        Utf8Path::new("/work"),
        "-W warnings",
        None,
        &[],
        Some("deletion"),
        None,
        &runner,
    )
    .unwrap();

    assert_eq!(note.ran, 3);
    let note = note.note;
    assert!(
        note.starts_with(
            "Browser tests: 1 passed, 2 failed, in a-ui. Running them needs \
             `wasm-bindgen-test-runner`"
        ),
        "{note}"
    );
    assert!(note.contains("--test highlight"), "{note}");
    assert_eq!(
        runner.calls()[1].args.last().map(String::as_str),
        Some("deletion"),
        "the name filter reaches the runner"
    );
}

/// The packages tested have no browser crate, so nothing runs in a browser.
#[test]
fn packages_without_a_browser_crate_run_no_browser_tests() {
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success(BROWSER_METADATA);

    let note = wasm_test(
        Utf8Path::new("/work"),
        "-W warnings",
        None,
        &["a".to_owned()],
        None,
        None,
        &runner,
    );

    assert_eq!(note, None);
    assert_eq!(runner.calls().len(), 1);
}

/// A filter naming no browser test runs nothing there, which says so rather
/// than reporting zero passed.
#[test]
fn a_filter_matching_no_browser_test_says_so() {
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success(BROWSER_METADATA)
        .expect("cargo")
        .returns_success("test result: ok. 0 passed; 0 failed; 0 ignored; 3 filtered out\n");

    let note = wasm_test(
        Utf8Path::new("/work"),
        "-W warnings",
        None,
        &[],
        Some("only_on_the_host"),
        None,
        &runner,
    );

    assert_eq!(
        note,
        Some(BrowserRun {
            note: "No browser test matched, in a-ui.".to_owned(),
            ran: 0,
        })
    );
}

fn host_run_of(code: i32) -> Result<HostRun, Failure> {
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "no tests to run\n".to_owned(),
            status: ExitCode::from_code(code),
        });

    host_run(
        Utf8Path::new("/work"),
        "-W warnings",
        None,
        &[],
        Some("seated".to_owned()),
        false,
        false,
        &runner,
    )
}

/// Nextest exits with 4 when its filter selects no test, and with 101 when the
/// build fails.
#[test]
fn a_host_run_that_matched_nothing_is_told_apart_from_a_failed_build() {
    assert!(matches!(host_run_of(4), Ok(HostRun::NoneMatched(_))));
    assert!(host_run_of(101).is_err());
}

fn browser_passed() -> BrowserRun {
    BrowserRun {
        note: "Browser tests: 2 passed, in a-ui.".to_owned(),
        ran: 2,
    }
}

/// A test named that exists only in the browser passes there, and the run
/// succeeds.
#[test]
fn a_run_matching_only_browser_tests_succeeds() {
    let host = Ok(HostRun::NoneMatched("Unable to run any tests.".to_owned()));

    let result = combine(host, Some(browser_passed()))
        .unwrap()
        .into_content()
        .unwrap();

    assert_eq!(
        result,
        "No host test matched.\n\nBrowser tests: 2 passed, in a-ui.\n"
    );
}

#[test]
fn a_failed_host_build_fails_the_run_whatever_the_browser_says() {
    let host = Err("Unable to run any tests.".into());

    let error = combine(host, Some(browser_passed()))
        .unwrap_err()
        .to_string();

    assert_eq!(
        error,
        "Unable to run any tests.\n\nBrowser tests: 2 passed, in a-ui."
    );
}

#[test]
fn a_run_matching_nothing_on_either_side_fails() {
    let host = Ok(HostRun::NoneMatched("Unable to run any tests.".to_owned()));
    let browser = Some(BrowserRun {
        note: "No browser test matched, in a-ui.".to_owned(),
        ran: 0,
    });

    let error = combine(host, browser).unwrap_err().to_string();

    assert_eq!(
        error,
        "Unable to run any tests.\n\nNo browser test matched, in a-ui."
    );
}

#[test]
fn the_browser_summary_sums_every_result_line() {
    let stdout = "test result: ok. 2 passed; 0 failed; 0 ignored\nnoise\ntest result: FAILED. 1 \
                  passed; 1 failed; 0 ignored\n";

    assert_eq!(browser_summary(stdout), (3, 1));
    assert_eq!(browser_summary(""), (0, 0));
}

#[test]
fn a_note_follows_what_the_run_says() {
    let passed: ToolResult = Ok("Ran 1/1 tests, of which 0 failed.\n".to_owned().into());

    let noted = with_note(passed, Some("Browser tests: 3 passed, in a-ui.".to_owned()))
        .unwrap()
        .into_content()
        .unwrap();

    assert_eq!(
        noted,
        "Ran 1/1 tests, of which 0 failed.\n\nBrowser tests: 3 passed, in a-ui.\n"
    );
}

#[test]
fn test_cargo_test_success() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    };

    let stdout = r#"{"type":"test","event":"ok","name":"my_test","stdout":""}"#;
    let runner = MockProcessRunner::success(stdout);

    let result = cargo_test_impl(
        &ctx.root,
        "-W warnings",
        None,
        &[],
        None,
        false,
        false,
        &runner,
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert_eq!(result, "Ran 1/1 tests, of which 0 failed.\n");
}

#[test]
fn test_cargo_test_with_failure() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    };

    let stdout = r#"{"type":"test","event":"failed","name":"my_crate$tests::my_test","stdout":"assertion failed"}"#;
    let runner = MockProcessRunner::success(stdout);

    let result = cargo_test_impl(
        &ctx.root,
        "-W warnings",
        None,
        &[],
        None,
        false,
        false,
        &runner,
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert_eq!(result, indoc::indoc! {"
            Ran 1/1 tests, of which 1 failed.

            What follows is an XML representation of the failed tests:

            ```xml
            <results>
                <test_failure>
                    <crate>my_crate</crate>
                    <path>tests::my_test</path>
                    <output>assertion failed</output>
                </test_failure>
            </results>
            ```"});
}

/// A test nextest kills for exceeding its slow-timeout is reported with a
/// reason and no captured output, which is exactly what nextest's libtest
/// reporter writes for it.
/// Counting it as passed would hide a hang behind a green summary.
#[test]
fn a_timed_out_test_is_reported_as_failed() {
    let stdout = indoc::indoc! {r#"
        {"type":"test","event":"started","name":"my_crate::my_crate$tests::slow"}
        {"type":"test","event":"failed","name":"my_crate::my_crate$tests::slow","exec_time":60.01,"reason":"time limit exceeded"}
        {"type":"test","event":"ok","name":"my_crate::my_crate$tests::fast","exec_time":0.01}
    "#};
    let runner = MockProcessRunner::builder()
        .expect_any()
        .returns(ProcessOutput {
            stdout: stdout.to_owned(),
            stderr: String::new(),
            status: ExitCode::from_code(100),
        });

    let content = cargo_test_impl(
        Utf8Path::new("/work"),
        "-W warnings",
        None,
        &[],
        None,
        false,
        false,
        &runner,
    )
    .unwrap()
    .unwrap_content();

    assert_eq!(content, indoc::indoc! {"
        Ran 2/2 tests, of which 1 failed.

        What follows is an XML representation of the failed tests:

        ```xml
        <results>
            <test_failure>
                <crate>my_crate</crate>
                <path>tests::slow</path>
                <reason>time limit exceeded</reason>
            </test_failure>
        </results>
        ```"});
}

/// Nextest failing the run without any failure the tool could parse is not a
/// green run: the output format changed, or something failed outside a test.
/// The exit code and stderr are what is left to go on.
#[test]
fn a_failed_run_with_no_parsed_failures_is_not_reported_as_passing() {
    let stdout =
        r#"{"type":"test","event":"ok","name":"my_crate::my_crate$tests::fast","exec_time":0.01}"#;
    let runner = MockProcessRunner::builder()
        .expect_any()
        .returns(ProcessOutput {
            stdout: stdout.to_owned(),
            stderr: "error: test run failed\n".to_owned(),
            status: ExitCode::from_code(100),
        });

    let content = cargo_test_impl(
        Utf8Path::new("/work"),
        "-W warnings",
        None,
        &[],
        None,
        false,
        false,
        &runner,
    )
    .unwrap()
    .unwrap_content();

    assert_eq!(content, indoc::indoc! {"
        Ran 1/1 tests, of which 0 failed.

        However, nextest exited with status 100, so the run did not succeed even though no \
        failing test was reported. Its error output follows:

        error: test run failed
    "});
}

#[test]
fn no_tests_ran_error_is_bounded() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    };

    // A panicking proc-macro derive produces one diagnostic per expansion site,
    // which for a widely-derived macro means megabytes of stderr.
    let stderr = "error: proc-macro derive panicked\n".repeat(10_000);
    let runner = MockProcessRunner::builder()
        .expect_any()
        .returns_error(&stderr);

    let error = cargo_test_impl(
        &ctx.root,
        "-W warnings",
        None,
        &[],
        None,
        false,
        false,
        &runner,
    )
    .expect_err("a run with zero tests is an error")
    .to_string();

    assert!(
        error.len() < MAX_DIAGNOSTIC_BYTES + 200,
        "error grew to {} bytes",
        error.len()
    );
    assert!(
        error.ends_with(&format!(
            "[Truncated: showing {MAX_DIAGNOSTIC_BYTES} of {} bytes]",
            stderr.len()
        )),
        "got: {}",
        &error[error.len() - 100..]
    );
}

#[test]
fn failure_output_is_bounded_across_the_whole_run() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    };

    // A broken shared fixture fails every test in the workspace, each one
    // carrying its own captured output.
    let per_test_output = "x".repeat(MAX_TEST_OUTPUT_BYTES);
    let stdout = (0..500)
        .map(|i| {
            format!(
                r#"{{"type":"test","event":"failed","name":"my_crate$tests::t{i}","stdout":"{per_test_output}"}}"#
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let runner = MockProcessRunner::success(stdout);

    let content = cargo_test_impl(
        &ctx.root,
        "-W warnings",
        None,
        &[],
        None,
        false,
        false,
        &runner,
    )
    .unwrap()
    .unwrap_content();

    // Every failure is still counted, even though most carry no output.
    assert!(
        content.starts_with("Ran 500/500 tests, of which 500 failed.\n"),
        "got: {}",
        &content[..60]
    );
    assert!(
        content.len() < MAX_TEST_OUTPUT_BUDGET_BYTES * 2,
        "response grew to {} bytes",
        content.len()
    );
    assert!(
        content.ends_with(
            "Output for 496 further failing tests was omitted to bound the size of this response. \
             Re-run with `testname` set to inspect them."
        ),
        "got tail: {}",
        &content[content.len() - 200..]
    );
}

#[test]
fn failure_blocks_are_bounded_when_captured_output_is_empty() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    };

    // Failing tests that print nothing still cost a serialized block each. If
    // only captured output were charged against the budget, none of these would
    // spend anything and every one would be serialized.
    let stdout = (0..5_000)
        .map(|i| {
            format!(
                r#"{{"type":"test","event":"failed","name":"my_crate$tests::t{i:04}","stdout":""}}"#
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let runner = MockProcessRunner::success(stdout);

    let content = cargo_test_impl(
        &ctx.root,
        "-W warnings",
        None,
        &[],
        None,
        false,
        false,
        &runner,
    )
    .unwrap()
    .unwrap_content();

    assert!(
        content.starts_with("Ran 5000/5000 tests, of which 5000 failed.\n"),
        "got: {}",
        &content[..60]
    );
    assert!(
        content.len() < MAX_TEST_OUTPUT_BUDGET_BYTES * 2,
        "response grew to {} bytes",
        content.len()
    );
    assert!(
        content.contains("further failing tests was omitted to bound the size of this response"),
        "got tail: {}",
        &content[content.len() - 200..]
    );
}

/// The cargo profile has to reach nextest under its own flag, and the
/// positional filter has to stay last.
///
/// Nextest's `--profile` selects one of *its* profiles, so a cargo profile
/// passed under that name would be silently misread as a nextest one.
#[test]
fn cargo_profile_is_passed_through_to_nextest() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    };

    let stdout = r#"{"type":"test","event":"ok","name":"my_test","stdout":""}"#;
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&[
            "nextest",
            "run",
            "--workspace",
            "--cargo-quiet",
            "--no-fail-fast",
            "--hide-progress-bar",
            "--final-status-level=none",
            "--status-level=fail",
            "--message-format=libtest-json-plus",
            "--cargo-profile=agent",
            "my_test",
        ])
        .returns_success(stdout);

    let content = cargo_test_impl(
        &ctx.root,
        "-W warnings",
        Some("agent"),
        &[],
        Some("my_test".to_owned()),
        false,
        false,
        &runner,
    )
    .unwrap()
    .unwrap_content();

    assert_eq!(content, "Ran 1/1 tests, of which 0 failed.\n");
}

/// Every named package reaches nextest as its own flag, so one run builds and
/// tests them together, with the name filter applying across all of them.
#[test]
fn each_package_is_passed_to_nextest() {
    let stdout = r#"{"type":"test","event":"ok","name":"my_test","stdout":""}"#;
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&[
            "nextest",
            "run",
            "--package=jp_storage",
            "--package=jp_workspace",
            "--cargo-quiet",
            "--no-fail-fast",
            "--hide-progress-bar",
            "--final-status-level=none",
            "--status-level=fail",
            "--message-format=libtest-json-plus",
            "my_test",
        ])
        .returns_success(stdout);

    let content = cargo_test_impl(
        Utf8Path::new("/work"),
        "-W warnings",
        None,
        &["jp_storage".to_owned(), "jp_workspace".to_owned()],
        Some("my_test".to_owned()),
        false,
        false,
        &runner,
    )
    .unwrap()
    .unwrap_content();

    assert_eq!(content, "Ran 1/1 tests, of which 0 failed.\n");
}

/// The variables the last command `runner` ran was given.
fn captured_env(runner: &MockProcessRunner) -> Vec<(String, String)> {
    runner.calls().pop().map_or_default(|call| call.env)
}

/// `cargo test` was the one compiling tool that never set `RUSTFLAGS`, so it
/// inherited `rustflags` from `.cargo/config.toml` while its siblings overrode
/// them.
/// That both thrashed a shared target directory and, in one workspace, applied
/// a flag that broke proc-macro crates outright.
#[test]
fn test_rustflags_reaches_cargo() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    };

    let stdout = r#"{"type":"test","event":"ok","name":"my_test","stdout":""}"#;
    let runner = MockProcessRunner::success(stdout);
    let _result = cargo_test_impl(
        &ctx.root,
        "-W warnings -Zthreads=0",
        None,
        &[],
        None,
        false,
        false,
        &runner,
    )
    .unwrap();

    assert_eq!(
        captured_env(&runner)
            .iter()
            .find(|(k, _)| k == "RUSTFLAGS")
            .map(|(_, v)| v.as_str()),
        Some("-W warnings -Zthreads=0"),
        "the merged flags must reach cargo, or `.cargo/config.toml` silently wins",
    );
}

/// A bare `insta` assertion resolves snapshots against the `CARGO_MANIFEST_DIR`
/// compiled into the test binary.
/// With a target directory shared across git worktrees, that binary may have
/// been built by a sibling worktree, so every snapshot reads as new unless the
/// root is named at runtime.
#[test]
fn insta_workspace_root_names_the_checkout_under_test() {
    let stdout = r#"{"type":"test","event":"ok","name":"my_test","stdout":""}"#;
    let runner = MockProcessRunner::success(stdout);
    let _result = cargo_test_impl(
        Utf8Path::new("/work/jp.git/bookworm-tool-def"),
        "-W warnings",
        None,
        &[],
        None,
        false,
        false,
        &runner,
    )
    .unwrap();

    assert_eq!(
        captured_env(&runner)
            .iter()
            .find(|(k, _)| k == "INSTA_WORKSPACE_ROOT")
            .map(|(_, v)| v.as_str()),
        Some("/work/jp.git/bookworm-tool-def"),
    );
}

#[test]
fn test_backtrace_disabled_by_default() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    };

    let stdout = r#"{"type":"test","event":"ok","name":"my_test","stdout":""}"#;
    let runner = MockProcessRunner::success(stdout);
    let _result = cargo_test_impl(
        &ctx.root,
        "-W warnings",
        None,
        &[],
        None,
        false,
        false,
        &runner,
    )
    .unwrap();

    assert_eq!(
        captured_env(&runner)
            .iter()
            .find(|(k, _)| k == "RUST_BACKTRACE")
            .map(|(_, v)| v.as_str()),
        Some("0"),
    );
}

#[test]
fn test_checksum_freshness_disabled_by_default() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    };

    let stdout = r#"{"type":"test","event":"ok","name":"my_test","stdout":""}"#;
    let runner = MockProcessRunner::success(stdout);
    let _result = cargo_test_impl(
        &ctx.root,
        "-W warnings",
        None,
        &[],
        None,
        false,
        false,
        &runner,
    )
    .unwrap();

    assert!(
        !captured_env(&runner)
            .iter()
            .any(|(k, _)| k == "CARGO_UNSTABLE_CHECKSUM_FRESHNESS"),
        "checksum freshness must be off unless opted into, so the tools work on stable cargo",
    );
}

#[test]
fn test_checksum_freshness_enabled() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    };

    let stdout = r#"{"type":"test","event":"ok","name":"my_test","stdout":""}"#;
    let runner = MockProcessRunner::success(stdout);
    let _result = cargo_test_impl(
        &ctx.root,
        "-W warnings",
        None,
        &[],
        None,
        false,
        true,
        &runner,
    )
    .unwrap();

    assert_eq!(
        captured_env(&runner)
            .iter()
            .find(|(k, _)| k == "CARGO_UNSTABLE_CHECKSUM_FRESHNESS")
            .map(|(_, v)| v.as_str()),
        Some("true"),
    );
}

#[test]
fn test_backtrace_enabled() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    };

    let stdout = r#"{"type":"test","event":"ok","name":"my_test","stdout":""}"#;
    let runner = MockProcessRunner::success(stdout);
    let _result = cargo_test_impl(
        &ctx.root,
        "-W warnings",
        None,
        &[],
        None,
        true,
        false,
        &runner,
    )
    .unwrap();

    assert_eq!(
        captured_env(&runner)
            .iter()
            .find(|(k, _)| k == "RUST_BACKTRACE")
            .map(|(_, v)| v.as_str()),
        Some("1"),
    );
}
