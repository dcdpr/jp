use jp_tool::Context;
use serde_json::{Value, from_str};

use super::MAX_DIAGNOSTIC_BYTES;
use crate::{
    to_simple_xml_with_root,
    util::{
        ToolResult,
        runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
        truncate,
    },
};

/// Cap for a single failing test's captured output.
///
/// Tighter than [`MAX_DIAGNOSTIC_BYTES`] because a run can report many
/// failures, and each one contributes its own block.
const MAX_TEST_OUTPUT_BYTES: usize = 8_000;

/// Cap for the captured output of all failing tests combined.
///
/// One broken fixture can fail every test in the workspace, so a per-failure
/// cap alone leaves the total unbounded.
/// Failures past this budget are counted and named in the summary but carry no
/// output.
const MAX_TEST_OUTPUT_BUDGET_BYTES: usize = 32_000;

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
    backtrace: Option<bool>,
    checksum_freshness: bool,
) -> ToolResult {
    cargo_test_impl(
        ctx,
        package,
        testname,
        backtrace.unwrap_or(false),
        checksum_freshness,
        &DuctProcessRunner,
    )
}

fn cargo_test_impl<R: ProcessRunner>(
    ctx: &Context,
    package: Option<String>,
    testname: Option<String>,
    backtrace: bool,
    checksum_freshness: bool,
    runner: &R,
) -> ToolResult {
    let test_name = testname.unwrap_or_default();
    let package = package.map_or("--workspace".to_owned(), |v| format!("--package={v}"));

    let mut env = vec![
        ("NEXTEST_EXPERIMENTAL_LIBTEST_JSON", "1"),
        ("RUST_BACKTRACE", if backtrace { "1" } else { "0" }),
    ];
    if checksum_freshness {
        // Use content checksums instead of file mtimes for cargo's freshness
        // checks, so that sibling checkouts (git worktrees) sharing a target
        // dir cannot serve each other's stale artifacts. Matches CI. Requires
        // nightly cargo. See rust-lang/cargo#14136.
        env.push(("CARGO_UNSTABLE_CHECKSUM_FRESHNESS", "true"));
    }

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
        &env,
    )?;

    let mut total_tests = 0;
    let mut ran_tests = 0;
    let mut failed_tests = 0;
    let mut spent_bytes = 0;
    let mut failure = vec![];
    for l in stdout.lines().filter_map(|s| from_str::<Value>(s).ok()) {
        let kind = l.get("type").and_then(Value::as_str).unwrap_or_default();
        let event = l.get("event").and_then(Value::as_str).unwrap_or_default();

        if kind != "test" || event == "started" {
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

        failed_tests += 1;
        if spent_bytes >= MAX_TEST_OUTPUT_BUDGET_BYTES {
            continue;
        }

        let output = truncate(stdout, MAX_TEST_OUTPUT_BYTES);
        spent_bytes += output.len();
        failure.push(TestFailure {
            krate: krate.to_owned(),
            path: path.to_owned(),
            output,
        });
    }

    if ran_tests == 0 {
        Err(format!(
            "Unable to run any tests. This can be due to compilation issues, or incorrect package \
             or test name:\n\n{}",
            truncate(&stderr, MAX_DIAGNOSTIC_BYTES)
        ))?;
    }

    let mut response =
        format!("Ran {ran_tests}/{total_tests} tests, of which {failed_tests} failed.\n");

    if !failure.is_empty() {
        let xml = to_simple_xml_with_root(&failure, "results")?;
        response.push_str("\nWhat follows is an XML representation of the failed tests:\n\n");
        response.push_str(&format!("```xml\n{xml}\n```"));

        let omitted = failed_tests - failure.len();
        if omitted > 0 {
            response.push_str(&format!(
                "\n\nOutput for {omitted} further failing tests was omitted to bound the size of \
                 this response. Re-run with `testname` set to inspect them."
            ));
        }
    }

    Ok(response.into())
}

#[cfg(test)]
#[path = "test_tests.rs"]
mod tests;
