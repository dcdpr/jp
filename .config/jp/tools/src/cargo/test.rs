use std::{env, error::Error};

use camino::Utf8Path;
use jp_process::{ProcessOutput, ProcessRunner, SystemProcessRunner};
use jp_tool::Outcome;
use serde_json::{Value, from_str};

use super::{
    CHECKSUM_FRESHNESS_ENV, MAX_DIAGNOSTIC_BYTES, WASM_TARGET, browser_crates, cargo_scope,
};
use crate::{
    to_simple_xml_with_root,
    util::{ToolResult, truncate},
};

/// What runs a browser crate's tests in a headless browser, as the cargo runner
/// of [`WASM_TARGET`].
const WASM_RUNNER: &str = "wasm-bindgen-test-runner";

/// The variables through which [`WASM_RUNNER`] is told which browser to drive,
/// and with which driver.
const DRIVER_VARIABLES: [&str; 8] = [
    "CHROMEDRIVER",
    "GECKODRIVER",
    "SAFARIDRIVER",
    "MSEDGEDRIVER",
    "CHROMEDRIVER_REMOTE",
    "GECKODRIVER_REMOTE",
    "SAFARIDRIVER_REMOTE",
    "MSEDGEDRIVER_REMOTE",
];

/// Exit code returned by nextest when its filter selects no tests,
/// distinguishing an empty filter match from a build failure.
const NO_TESTS_RUN: i32 = 4;

type Failure = Box<dyn Error + Send + Sync>;

/// Outcome of a nextest run that did not fail to build or execute.
#[derive(Debug)]
enum HostRun {
    Ran(Outcome),

    /// Build succeeded, but the filter matched no tests.
    /// Carries the error message to report if no browser tests matched either.
    NoneMatched(String),
}

impl HostRun {
    /// Convert into a [`ToolResult`], treating an empty filter match as an
    /// error.
    fn into_result(self) -> ToolResult {
        match self {
            Self::Ran(outcome) => Ok(outcome),
            Self::NoneMatched(error) => Err(error.into()),
        }
    }
}

/// Summary of a headless browser test run.
#[derive(Debug, PartialEq, Eq)]
struct BrowserRun {
    note: String,
    ran: usize,
}

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

/// Run the tests of `packages`, or of the whole workspace, through nextest;
/// with `wasm`, run the browser crates' browser tests as well, and report them
/// in a note.
pub(crate) async fn cargo_test(
    root: &Utf8Path,
    rustflags: &str,
    profile: Option<&str>,
    packages: &[String],
    testname: Option<String>,
    backtrace: Option<bool>,
    checksum_freshness: bool,
    wasm: bool,
) -> ToolResult {
    let browsers = if wasm {
        let found = which::which("chromedriver")
            .ok()
            .and_then(|path| path.into_os_string().into_string().ok());
        Browsers::Run {
            chromedriver: chromedriver_for(|name| env::var_os(name).is_some(), found),
        }
    } else {
        Browsers::Skip
    };

    cargo_test_impl(
        root,
        rustflags,
        profile,
        packages,
        testname,
        backtrace.unwrap_or(false),
        checksum_freshness,
        browsers,
        &SystemProcessRunner,
    )
}

/// Whether to run browser integration tests in addition to host tests.
#[derive(Debug)]
enum Browsers {
    Skip,
    Run { chromedriver: Option<String> },
}

/// Combine the host and browser test outcomes into a single [`ToolResult`].
///
/// A test name filter may match tests on only one side (for example, a filter
/// naming a browser-only integration test).
/// The run succeeds if at least one test ran on either side, and fails if
/// neither side matched any tests.
fn combine(host: Result<HostRun, Failure>, browser: Option<BrowserRun>) -> ToolResult {
    let Some(browser) = browser else {
        return host.and_then(HostRun::into_result);
    };

    match host {
        Ok(HostRun::NoneMatched(_)) if browser.ran > 0 => {
            Ok(format!("No host test matched.\n\n{}\n", browser.note).into())
        }
        host => with_note(host.and_then(HostRun::into_result), Some(browser.note)),
    }
}

/// The `CHROMEDRIVER` to hand [`WASM_RUNNER`]: `found`, the `chromedriver` on
/// the `PATH`, unless one of [`DRIVER_VARIABLES`] is `set`.
///
/// Chrome is what CI runs the browser tests in.
/// Without a variable, the runner takes the first of `geckodriver`,
/// `safaridriver`, and `chromedriver` it finds on the `PATH`, so a machine with
/// Firefox installed runs them there.
fn chromedriver_for(set: impl Fn(&str) -> bool, found: Option<String>) -> Option<String> {
    if DRIVER_VARIABLES.iter().any(|name| set(name)) {
        return None;
    }

    found
}

/// `outcome`, with `note` after what it says.
fn with_note(outcome: ToolResult, note: Option<String>) -> ToolResult {
    let Some(note) = note else {
        return outcome;
    };

    match outcome {
        Ok(Outcome::Success { content }) => Ok(Outcome::Success {
            content: format!("{content}\n{note}\n"),
        }),
        Ok(Outcome::Error {
            message,
            trace,
            transient,
        }) => Ok(Outcome::Error {
            message: format!("{message}\n\n{note}"),
            trace,
            transient,
        }),
        Err(error) => Err(format!("{error}\n\n{note}").into()),
        other => other,
    }
}

/// Run the browser tests for the browser crates in `packages` (or all browser
/// crates when `packages` is empty) in a headless browser via [`WASM_RUNNER`].
///
/// Returns `None` when there are no browser crates to test.
/// Only the integration tests in `tests/` run in the browser; unit tests in a
/// browser crate contain pure logic covered by the host test run.
/// `cargo test` drives the runner directly because nextest does not support it.
/// When `chromedriver` is set, tests run in headless Chrome via that binary.
fn wasm_test<R: ProcessRunner>(
    root: &Utf8Path,
    rustflags: &str,
    profile: Option<&str>,
    packages: &[String],
    testname: Option<&str>,
    chromedriver: Option<&str>,
    runner: &R,
) -> Option<BrowserRun> {
    let unrun = |note: String| Some(BrowserRun { note, ran: 0 });
    let crates = match browser_crates(root, packages, runner) {
        Ok(crates) if crates.is_empty() => return None,
        Ok(crates) => crates,
        Err(reason) => return unrun(format!("The browser tests weren't run: {reason}")),
    };

    let scope = cargo_scope(&crates, "--workspace");
    let profile = profile.map(|name| format!("--profile={name}"));
    // No `--quiet`: cargo hands it on to the test runner, and
    // `wasm-bindgen-test-runner` takes no flags.
    let mut args = vec!["test", "--target", WASM_TARGET, "--test", "*"];
    args.extend(scope.iter().map(String::as_str));
    args.extend(profile.as_deref());
    if let Some(name) = testname {
        args.extend(["--", name]);
    }
    let mut env = vec![
        ("RUSTFLAGS", rustflags),
        ("CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER", WASM_RUNNER),
    ];
    if let Some(path) = chromedriver {
        env.push(("CHROMEDRIVER", path));
    }

    let output = match runner.run_with_env("cargo", &args, root, &env) {
        Ok(output) => output,
        Err(error) => {
            return unrun(format!(
                "The browser tests weren't run: `cargo` couldn't be run: {error}"
            ));
        }
    };
    let (passed, failed) = browser_summary(&output.stdout);
    let ran = passed + failed;
    if output.status.is_success() && ran == 0 {
        return unrun(format!(
            "No browser test matched, in {}.",
            crates.join(", ")
        ));
    }
    if output.status.is_success() {
        return Some(BrowserRun {
            note: format!("Browser tests: {passed} passed, in {}.", crates.join(", ")),
            ran,
        });
    }

    let log = truncate(
        format!("{}\n{}", output.stdout.trim(), output.stderr.trim()).trim(),
        MAX_DIAGNOSTIC_BYTES,
    );
    Some(BrowserRun {
        note: format!(
            "Browser tests: {passed} passed, {failed} failed, in {}. Running them needs \
             `{WASM_RUNNER}` (`just _install-wasm-bindgen`) and a WebDriver: `chromedriver` on \
             the `PATH`, or the one `CHROMEDRIVER`, `GECKODRIVER`, or `SAFARIDRIVER` names. The \
             log says which browser ran.\n```\n{log}\n```",
            crates.join(", ")
        ),
        ran,
    })
}

/// How many tests passed and failed, summed over each `test result:` line
/// `stdout` holds.
fn browser_summary(stdout: &str) -> (usize, usize) {
    let count = |line: &str, word: &str| {
        line.split(';')
            .find_map(|part| part.trim().strip_suffix(word))
            .and_then(|number| number.trim().rsplit(' ').next()?.parse::<usize>().ok())
            .unwrap_or(0)
    };

    stdout
        .lines()
        .filter(|line| line.starts_with("test result:"))
        .fold((0, 0), |(passed, failed), line| {
            (
                passed + count(line, "passed"),
                failed + count(line, "failed"),
            )
        })
}

/// Run host tests via nextest and optionally browser tests according to
/// `browsers`, failing if neither side matches any tests.
fn cargo_test_impl<R: ProcessRunner>(
    root: &Utf8Path,
    rustflags: &str,
    profile: Option<&str>,
    packages: &[String],
    testname: Option<String>,
    backtrace: bool,
    checksum_freshness: bool,
    browsers: Browsers,
    runner: &R,
) -> ToolResult {
    let filter = testname.clone();
    let host = host_run(
        root,
        rustflags,
        profile,
        packages,
        testname,
        backtrace,
        checksum_freshness,
        runner,
    );
    let browser = match browsers {
        Browsers::Skip => None,
        Browsers::Run { chromedriver } => wasm_test(
            root,
            rustflags,
            profile,
            packages,
            filter.as_deref(),
            chromedriver.as_deref(),
            runner,
        ),
    };

    combine(host, browser)
}

/// Run host tests via nextest.
///
/// Returns [`HostRun::NoneMatched`] when nextest exits with [`NO_TESTS_RUN`],
/// or an error if nextest cannot be spawned or the build fails.
fn host_run<R: ProcessRunner>(
    root: &Utf8Path,
    rustflags: &str,
    profile: Option<&str>,
    packages: &[String],
    testname: Option<String>,
    backtrace: bool,
    checksum_freshness: bool,
    runner: &R,
) -> Result<HostRun, Failure> {
    let test_name = testname.unwrap_or_default();
    let scope = cargo_scope(packages, "--workspace");
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
        env.extend(CHECKSUM_FRESHNESS_ENV);
    }

    let mut args = vec!["nextest", "run"];
    args.extend(scope.iter().map(String::as_str));
    args.extend([
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
    ]);
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
        let error = format!(
            "Unable to run any tests. This can be due to compilation issues, or incorrect package \
             or test name:\n\n{}",
            truncate(&stderr, MAX_DIAGNOSTIC_BYTES)
        );
        if status.code() == Some(NO_TESTS_RUN) {
            return Ok(HostRun::NoneMatched(error));
        }
        return Err(error.into());
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

    Ok(HostRun::Ran(response.into()))
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
