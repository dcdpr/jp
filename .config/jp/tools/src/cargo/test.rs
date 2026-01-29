use jp_tool::Context;
use serde_json::{Value, from_str};

use crate::{
    to_simple_xml_with_root,
    util::{
        ToolResult,
        runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
    },
};

#[derive(serde::Serialize)]
struct TestFailure {
    #[serde(rename = "crate")]
    krate: String,
    path: String,
    output: String,
}

pub(crate) async fn cargo_test(
    ctx: &Context,
    package: Option<String>,
    testname: Option<String>,
) -> ToolResult {
    cargo_test_impl(ctx, package, testname, &DuctProcessRunner)
}

fn cargo_test_impl<R: ProcessRunner>(
    ctx: &Context,
    package: Option<String>,
    testname: Option<String>,
    runner: &R,
) -> ToolResult {
    let test_name = testname.unwrap_or_default();
    let package = package.map_or("--workspace".to_owned(), |v| format!("--package={v}"));

    let ProcessOutput { stdout, stderr, .. } = runner.run_with_env(
        "cargo",
        &[
            "nextest",
            "run",
            &package,
            // Once to still print any compilation errors.
            "--cargo-quiet",
            // Run all tests, even if one fails.
            "--no-fail-fast",
            // Dense output for better LLM readability.
            "--hide-progress-bar",
            "--final-status-level=none",
            "--status-level=fail",
            // JSON output to be parsed by the tool.
            "--message-format=libtest-json-plus",
            &test_name,
        ],
        &ctx.root,
        &[
            ("NEXTEST_EXPERIMENTAL_LIBTEST_JSON", "1"),
            ("RUST_BACKTRACE", "1"),
        ],
    )?;

    let mut total_tests = 0;
    let mut ran_tests = 0;
    let mut failure = vec![];
    for l in stdout.lines().filter_map(|s| from_str::<Value>(s).ok()) {
        let kind = l.get("type").and_then(Value::as_str).unwrap_or_default();
        let event = l.get("event").and_then(Value::as_str).unwrap_or_default();

        if kind != "test" {
            continue;
        }
        total_tests += 1;
        if event != "ignored" {
            ran_tests += 1;
        }
        if event != "failed" {
            continue;
        }

        let Some(name) = l.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(stdout) = l.get("stdout").and_then(Value::as_str) else {
            continue;
        };

        let (krate, path) = name.split_once('$').unwrap_or(("", name));
        let krate = krate.split_once("::").unwrap_or((krate, "")).0;

        failure.push(TestFailure {
            krate: krate.to_owned(),
            path: path.to_owned(),
            output: stdout.to_owned(),
        });
    }

    if ran_tests == 0 {
        Err(format!(
            "Unable to run any tests. This can be due to compilation issues, or incorrect package \
             or test name:\n\n{stderr}"
        ))?;
    }

    let mut response = format!(
        "Ran {ran_tests}/{total_tests} tests, of which {} failed.\n",
        failure.len()
    );

    if !failure.is_empty() {
        let xml = to_simple_xml_with_root(&failure, "results")?;
        response.push_str("\nWhat follows is an XML representation of the failed tests:\n\n");
        response.push_str(&format!("```xml\n{xml}\n```"));
    }

    Ok(response.into())
}

#[cfg(test)]
mod tests {
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

        let result = cargo_test_impl(&ctx, None, None, &runner)
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

        let result = cargo_test_impl(&ctx, None, None, &runner)
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
}
