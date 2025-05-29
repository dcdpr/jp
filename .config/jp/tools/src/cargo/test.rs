use std::path::PathBuf;

// FIXME: clippy diagnostics not showing up in here?
use duct::cmd;
use indoc::formatdoc;
use mcp_attr::server::RequestContext;
use serde_json::{from_str, Value};

use crate::to_xml;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync + 'static>>;

pub(crate) async fn cargo_test(
    package: Option<String>,
    testname: Option<String>,
    ctx: &RequestContext,
) -> Result<String> {
    #[derive(serde::Serialize)]
    struct TestResult {
        total_tests: usize,
        failures: Vec<TestFailure>,
    }

    #[derive(serde::Serialize)]
    struct TestFailure {
        #[serde(rename = "crate")]
        krate: String,
        path: String,
        stdout: String,
    }

    let root = ctx
        .roots_list()
        .await?
        .iter()
        .find_map(|v| {
            v.name
                .as_ref()
                .is_some_and(|v| v.as_str() == "project")
                .then(|| v.to_file_path())
                .flatten()
        })
        .unwrap_or(PathBuf::from("."));

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
    .dir(root)
    .env("NEXTEST_EXPERIMENTAL_LIBTEST_JSON", "1")
    .env("RUST_BACKTRACE", "1")
    .unchecked()
    .read()?;

    let mut total_tests = 0;
    let mut failures = vec![];
    for l in result.lines().filter_map(|s| from_str::<Value>(s).ok()) {
        if l.get("type").and_then(Value::as_str) == Some("test") {
            total_tests += 1;
        } else {
            continue;
        }
        if l.get("event").and_then(Value::as_str) != Some("failed") {
            continue;
        }
        let Some(name) = l.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(stdout) = l.get("stdout").and_then(Value::as_str) else {
            continue;
        };

        let (krate, path) = name.split_once("$").unwrap_or(("", name));
        let krate = krate.split_once("::").unwrap_or((krate, "")).0;

        failures.push(TestFailure {
            krate: krate.to_owned(),
            path: path.to_owned(),
            stdout: stdout.to_owned(),
        });
    }

    Ok(formatdoc! {"
            The test run was completed successfully:

            {}
        ", to_xml(TestResult {
        total_tests,
        failures,
    })})
}
