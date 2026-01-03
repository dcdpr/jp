use duct::cmd;
use jp_tool::Context;
use serde_json::{Value, from_str};

use crate::{Result, to_xml};

#[derive(serde::Serialize)]
struct TestResult {
    failure: Vec<TestFailure>,
}

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
) -> Result<String> {
    let test_name = testname.unwrap_or_default();
    let package = package.map_or("--workspace".to_owned(), |v| format!("--package={v}"));
    let result = cmd!(
        "cargo",
        "nextest",
        "run",
        package,
        // Twice to silence Cargo completely.
        "--cargo-quiet",
        "--cargo-quiet",
        // Run all tests, even if one fails.
        "--no-fail-fast",
        // Dense output for better LLM readability.
        "--hide-progress-bar",
        "--final-status-level=none",
        "--status-level=fail",
        // JSON output to be parsed by the tool.
        "--message-format=libtest-json-plus",
        test_name
    )
    .dir(&ctx.root)
    .env("NEXTEST_EXPERIMENTAL_LIBTEST_JSON", "1")
    .env("RUST_BACKTRACE", "1")
    .stderr_capture()
    .stdout_capture()
    .unchecked()
    .run()?;

    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);

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

    let failed_tests = failure.len();

    if ran_tests == 0 {
        return Err(format!(
            "Unable to find any tests. Are the package and test name correct?\n\n{stderr}"
        ))?;
    }

    let mut response =
        format!("Ran {ran_tests}/{total_tests} tests, of which {failed_tests} failed.\n");
    if !failure.is_empty() {
        response.push_str("\nWhat follows is an XML representation of the failed tests:\n\n");
        response.push_str(&to_xml(TestResult { failure })?);
    }

    Ok(response)
}
