use std::{io, sync::Mutex};

use camino::Utf8Path;
use camino_tempfile::tempdir;
use jp_tool::Action;

use super::*;
use crate::util::runner::MockProcessRunner;

#[test]
fn test_cargo_test_success() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
    };

    let stdout = r#"{"type":"test","event":"ok","name":"my_test","stdout":""}"#;
    let runner = MockProcessRunner::success(stdout);

    let result = cargo_test_impl(&ctx, None, None, false, &runner)
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
    };

    let stdout = r#"{"type":"test","event":"failed","name":"my_crate$tests::my_test","stdout":"assertion failed"}"#;
    let runner = MockProcessRunner::success(stdout);

    let result = cargo_test_impl(&ctx, None, None, false, &runner)
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
    fn run_with_env_and_stdin(
        &self,
        program: &str,
        args: &[&str],
        working_dir: &Utf8Path,
        env: &[(&str, &str)],
        stdin: Option<&str>,
    ) -> Result<ProcessOutput, io::Error> {
        *self.captured_env.lock().unwrap() = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        self.inner
            .run_with_env_and_stdin(program, args, working_dir, env, stdin)
    }
}

#[test]
fn test_backtrace_disabled_by_default() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
    };

    let stdout = r#"{"type":"test","event":"ok","name":"my_test","stdout":""}"#;
    let runner: EnvCapturingRunner = MockProcessRunner::success(stdout).into();
    let _result = cargo_test_impl(&ctx, None, None, false, &runner).unwrap();

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
fn test_backtrace_enabled() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
    };

    let stdout = r#"{"type":"test","event":"ok","name":"my_test","stdout":""}"#;
    let runner: EnvCapturingRunner = MockProcessRunner::success(stdout).into();
    let _result = cargo_test_impl(&ctx, None, None, true, &runner).unwrap();

    assert_eq!(
        runner
            .captured_env()
            .iter()
            .find(|(k, _)| k == "RUST_BACKTRACE")
            .map(|(_, v)| v.as_str()),
        Some("1"),
    );
}
