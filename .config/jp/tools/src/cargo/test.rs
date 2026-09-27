use camino::Utf8Path;
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

/// Cap for the serialized failure blocks of a run, combined.
///
/// One broken fixture can fail every test in the workspace, so a per-failure
/// cap alone leaves the total unbounded.
/// Failures past this budget are counted and named in the summary but carry no
/// output.
const MAX_TEST_OUTPUT_BUDGET_BYTES: usize = 32_000;

/// Approximate size of one serialized failure block minus its captured output:
/// the XML tags and indentation around the crate, path, and output fields.
///
/// Charged against [`MAX_TEST_OUTPUT_BUDGET_BYTES`] so that failures with empty
/// captured output still consume budget.
/// Without it a run where every failure prints nothing spends nothing, and the
/// block scaffolding alone grows the response without bound.
const FAILURE_BLOCK_OVERHEAD_BYTES: usize = 120;

#[derive(serde::Serialize)]
struct TestFailure {
    #[serde(rename = "crate")]
    krate: String,
    path: String,

    /// Why nextest failed the test when it was not the test's own assertion,
    /// such as `time limit exceeded` for one it killed.
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,

    /// What the test printed; absent for a test nextest killed or failed on its
    /// own terms.
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
}

pub(crate) async fn cargo_test(
    root: &Utf8Path,
    rustflags: &str,
    profile: Option<&str>,
    package: Option<String>,
    testname: Option<String>,
    backtrace: Option<bool>,
    checksum_freshness: bool,
) -> ToolResult {
    cargo_test_impl(
        root,
        rustflags,
        profile,
        package,
        testname,
        backtrace.unwrap_or(false),
        checksum_freshness,
        &DuctProcessRunner,
    )
}

fn cargo_test_impl<R: ProcessRunner>(
    root: &Utf8Path,
    rustflags: &str,
    profile: Option<&str>,
    package: Option<String>,
    testname: Option<String>,
    backtrace: bool,
    checksum_freshness: bool,
    runner: &R,
) -> ToolResult {
    let test_name = testname.unwrap_or_default();
    let package = package.map_or("--workspace".to_owned(), |v| format!("--package={v}"));
    // `--profile` selects a nextest profile; the cargo one has its own flag.
    let profile_arg = profile.map(|name| format!("--cargo-profile={name}"));

    let mut env = vec![
        ("NEXTEST_EXPERIMENTAL_LIBTEST_JSON", "1"),
        ("RUST_BACKTRACE", if backtrace { "1" } else { "0" }),
        ("RUSTFLAGS", rustflags),
        // A bare `insta` assertion resolves snapshots against the
        // `CARGO_MANIFEST_DIR` compiled into the test binary. Worktrees sharing
        // a target directory can run a binary a sibling worktree built, and
        // every snapshot then reads as new. Matches `just test`.
        ("INSTA_WORKSPACE_ROOT", root.as_str()),
    ];
    if checksum_freshness {
        // Use content checksums instead of file mtimes for cargo's freshness
        // checks, so that sibling checkouts (git worktrees) sharing a target
        // dir cannot serve each other's stale artifacts. Matches CI. Requires
        // nightly cargo. See rust-lang/cargo#14136.
        env.push(("CARGO_UNSTABLE_CHECKSUM_FRESHNESS", "true"));
    }

    let mut args = vec![
        "nextest",
        "run",
        package.as_str(),
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
    ];
    if let Some(profile) = profile_arg.as_deref() {
        args.push(profile);
    }
    // The filter is positional, so it stays last.
    args.push(&test_name);

    let ProcessOutput {
        stdout,
        stderr,
        status,
    } = runner.run_with_env("cargo", &args, root, &env)?;

    let RunSummary {
        total_tests,
        ran_tests,
        failed_tests,
        failure,
    } = parse_run(&stdout);

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

    // Nextest exits non-zero whenever the run failed. With no failure parsed
    // above, the reason is somewhere the parse does not look (a changed output
    // format, a setup script, a binary that crashed outside any test), and
    // reporting the summary alone would read as a green run.
    if failed_tests == 0 && !status.is_success() {
        response.push_str(&format!(
            "\nHowever, nextest exited with status {status}, so the run did not succeed even \
             though no failing test was reported. Its error output follows:\n\n{}",
            truncate(&stderr, MAX_DIAGNOSTIC_BYTES)
        ));
    }

    Ok(response.into())
}

/// What nextest's `libtest-json-plus` output says about a run.
struct RunSummary {
    total_tests: usize,
    ran_tests: usize,
    failed_tests: usize,

    /// The failures to show, which is fewer than `failed_tests` once their
    /// combined size reaches [`MAX_TEST_OUTPUT_BUDGET_BYTES`].
    failure: Vec<TestFailure>,
}

fn parse_run(stdout: &str) -> RunSummary {
    let mut summary = RunSummary {
        total_tests: 0,
        ran_tests: 0,
        failed_tests: 0,
        failure: vec![],
    };
    let mut spent_bytes = 0;
    for l in stdout.lines().filter_map(|s| from_str::<Value>(s).ok()) {
        let kind = l.get("type").and_then(Value::as_str).unwrap_or_default();
        let event = l.get("event").and_then(Value::as_str).unwrap_or_default();

        if kind != "test" || event == "started" {
            continue;
        }
        summary.total_tests += 1;
        if event != "ignored" {
            summary.ran_tests += 1;
        }
        if event != "failed" {
            continue;
        }

        // Counted before anything else is read: nextest writes a test it
        // killed for exceeding its slow-timeout, and a flaky test configured to
        // fail, as `failed` with a `reason` and no `stdout`.
        summary.failed_tests += 1;
        if spent_bytes >= MAX_TEST_OUTPUT_BUDGET_BYTES {
            continue;
        }

        let name = l.get("name").and_then(Value::as_str).unwrap_or("<unnamed>");
        let (krate, path) = name.split_once('$').unwrap_or(("", name));
        let krate = krate.split_once("::").unwrap_or((krate, "")).0;
        let reason = l.get("reason").and_then(Value::as_str).map(str::to_owned);
        let output = l
            .get("stdout")
            .and_then(Value::as_str)
            .map(|stdout| truncate(stdout, MAX_TEST_OUTPUT_BYTES));

        spent_bytes += output.as_ref().map_or(0, String::len)
            + reason.as_ref().map_or(0, String::len)
            + name.len()
            + FAILURE_BLOCK_OVERHEAD_BYTES;
        summary.failure.push(TestFailure {
            krate: krate.to_owned(),
            path: path.to_owned(),
            reason,
            output,
        });
    }
    summary
}

#[cfg(test)]
#[path = "test_tests.rs"]
mod tests;
