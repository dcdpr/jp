use std::{io, sync::Mutex};

use camino::Utf8Path;
use camino_tempfile::tempdir;
use jp_tool::{Action, Context};

use super::*;
use crate::util::runner::{MockProcessRunner, RunnerOpts};

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

    let result = cargo_test_impl(&ctx.root, "-W warnings", None, None, false, false, &runner)
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

    let result = cargo_test_impl(&ctx.root, "-W warnings", None, None, false, false, &runner)
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

    let error = cargo_test_impl(&ctx.root, "-W warnings", None, None, false, false, &runner)
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

    let content = cargo_test_impl(&ctx.root, "-W warnings", None, None, false, false, &runner)
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

    let content = cargo_test_impl(&ctx.root, "-W warnings", None, None, false, false, &runner)
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

/// A runner that captures the environment variables passed to it, so we can
/// assert on the exact values.
struct EnvCapturingRunner {
    inner: MockProcessRunner,
    captured_env: Mutex<Vec<(String, String)>>,
}

impl From<MockProcessRunner> for EnvCapturingRunner {
    fn from(inner: MockProcessRunner) -> Self {
        Self {
            inner,
            captured_env: Mutex::new(Vec::new()),
        }
    }
}

impl EnvCapturingRunner {
    fn captured_env(&self) -> Vec<(String, String)> {
        self.captured_env.lock().unwrap().clone()
    }
}

impl ProcessRunner for EnvCapturingRunner {
    fn run_with_opts(
        &self,
        program: &str,
        args: &[&str],
        working_dir: &Utf8Path,
        opts: &RunnerOpts<'_>,
    ) -> Result<ProcessOutput, io::Error> {
        *self.captured_env.lock().unwrap() = opts
            .env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        self.inner.run_with_opts(program, args, working_dir, opts)
    }
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
    let runner: EnvCapturingRunner = MockProcessRunner::success(stdout).into();
    let _result = cargo_test_impl(
        &ctx.root,
        "-W warnings -Zthreads=0",
        None,
        None,
        false,
        false,
        &runner,
    )
    .unwrap();

    assert_eq!(
        runner
            .captured_env()
            .iter()
            .find(|(k, _)| k == "RUSTFLAGS")
            .map(|(_, v)| v.as_str()),
        Some("-W warnings -Zthreads=0"),
        "the merged flags must reach cargo, or `.cargo/config.toml` silently wins",
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
    let runner: EnvCapturingRunner = MockProcessRunner::success(stdout).into();
    let _result =
        cargo_test_impl(&ctx.root, "-W warnings", None, None, false, false, &runner).unwrap();

    assert_eq!(
        runner
            .captured_env()
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
    let runner: EnvCapturingRunner = MockProcessRunner::success(stdout).into();
    let _result =
        cargo_test_impl(&ctx.root, "-W warnings", None, None, false, false, &runner).unwrap();

    assert!(
        !runner
            .captured_env()
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
    let runner: EnvCapturingRunner = MockProcessRunner::success(stdout).into();
    let _result =
        cargo_test_impl(&ctx.root, "-W warnings", None, None, false, true, &runner).unwrap();

    assert_eq!(
        runner
            .captured_env()
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
    let runner: EnvCapturingRunner = MockProcessRunner::success(stdout).into();
    let _result =
        cargo_test_impl(&ctx.root, "-W warnings", None, None, true, false, &runner).unwrap();

    assert_eq!(
        runner
            .captured_env()
            .iter()
            .find(|(k, _)| k == "RUST_BACKTRACE")
            .map(|(_, v)| v.as_str()),
        Some("1"),
    );
}
