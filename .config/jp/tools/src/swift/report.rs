//! Turning a test runner's output into something worth reading.
//!
//! Shared by [`swift_test`] and [`swift_test_ui`], which run different bundles
//! through different filters but report their results the same way.
//!
//! [`swift_test_ui`]: super::test_ui
//! [`swift_test`]: super::test

use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use jp_tool::Context;
use serde_json::Value;

use super::strip;
use crate::util::runner::{ProcessOutput, ProcessRunner};

/// How much of the raw log to show when there is no summary to show instead.
const LOG_TAIL_LINES: usize = 40;

/// The app's unit-test bundle, hosted by the app process.
const UNIT_BUNDLE: &str = "JPTests";

/// The app's UI-test bundle, which drives the app from outside it.
pub(super) const UI_BUNDLE: &str = "JPUITests";

/// An `-only-testing` argument addressing the unit bundle.
///
/// `-only-testing` takes `<bundle>/<suite>/<test>`; the bundle alone narrows a
/// run to everything in it.
pub(super) fn unit_bundle_filter(testname: Option<&str>) -> String {
    match testname {
        Some(name) => format!("-only-testing:{UNIT_BUNDLE}/{name}"),
        None => format!("-only-testing:{UNIT_BUNDLE}"),
    }
}

/// An `-only-testing` argument addressing one test or suite in the UI bundle.
pub(super) fn ui_bundle_filter(test: &str) -> String {
    format!("-only-testing:{UI_BUNDLE}/{test}")
}

/// The labelled summary of a run, or the message explaining why it is not one.
///
/// A run that reported no summary is not a passing run, it is a run that did
/// nothing: every runner here can exit zero when a filter matches nothing at
/// all.
pub(super) fn outcome(output: &ProcessOutput, label: &str) -> Result<String, String> {
    // Scan the whole log before capping it. A non-quiet run is far longer than
    // the diagnostic cap and the summary is at the end, so truncating first would
    // throw away the only lines worth reading.
    let log = Log::from(output);
    let summary = log.summary();

    if !output.status.is_success() {
        let detail = [log.crashes(), log.failures(), summary]
            .into_iter()
            .filter(|section| !section.is_empty())
            .collect::<Vec<_>>()
            .join("\n");

        if !detail.is_empty() {
            return Err(format!(
                "{label} tests failed:\n\n```\n{}\n```",
                strip(&detail)
            ));
        }

        // Neither a failing test nor a summary means the run did not get far enough
        // to report either. Showing the head of the log here would show the build
        // starting; whatever went wrong is at the other end.
        return Err(format!(
            "{label} tests failed without reporting a failing test or a summary, so the run died \
             rather than failing. The end of its output:\n\n```\n{}\n```",
            log.tail(LOG_TAIL_LINES)
        ));
    }

    if summary.is_empty() {
        return Err(format!(
            "{label} exited successfully but reported no test summary, so no test ran. A name \
             that matches nothing in the bundle it was pointed at will do this. The end of its \
             output:\n\n```\n{}\n```",
            log.tail(LOG_TAIL_LINES)
        ));
    }

    // A run can cover more than one bundle and so report more than one summary.
    // Labelling each keeps an unlabelled second line from reading as output
    // from somewhere else.
    Ok(summary
        .lines()
        .map(|line| format!("{label}: {line}"))
        .collect::<Vec<_>>()
        .join("\n"))
}

/// The UI test bundle's identifier, as `apps/macos/project.yml` sets it.
const UI_BUNDLE_ID: &str = "computer.jp.jean-pierre.uitests";

/// Where collected screenshots are put, relative to the repository root.
const SCREENSHOT_DIR: &str = "tmp/uitests";

/// Where the UI test runner writes its failure screenshots.
///
/// Xcode wraps a UI test bundle in a generated, sandboxed runner app, so the
/// tests cannot write into the checkout and put their screenshots in the
/// container's temporary directory instead.
/// Nothing reports that path, so it is derived the same way the runner does:
/// from its bundle identifier, which is the test bundle's with `.xctrunner`
/// appended.
fn screenshot_source() -> Option<Utf8PathBuf> {
    let home = std::env::var("HOME").ok()?;

    Some(Utf8PathBuf::from(home).join(format!(
        "Library/Containers/{UI_BUNDLE_ID}.xctrunner/Data/tmp/jp-uitests"
    )))
}

/// Discard screenshots left by an earlier run.
pub(super) fn clear_screenshots() {
    if let Some(dir) = screenshot_source() {
        let _removed = fs::remove_dir_all(&dir);
    }
}

/// The file the UI tests write their failure messages to.
const FAILURE_LOG: &str = "failures.txt";

/// Where `xcodebuild` is told to write the result bundle for a UI run.
///
/// Inside the checkout and under `tmp/`, so it is reachable without deriving a
/// container path and is thrown away with the rest of the scratch directory.
pub(super) const RESULT_BUNDLE: &str = "tmp/uitests/run.xcresult";

/// Discard the result bundle an earlier run left behind.
///
/// `xcodebuild` refuses to write over an existing bundle, so this is not
/// tidying up: without it the second UI run in a checkout fails before it
/// starts.
pub(super) fn clear_result_bundle(root: &Utf8Path) {
    let _removed = fs::remove_dir_all(root.join(RESULT_BUNDLE));
}

/// The file the test runner's own output is staged into.
const RUNNER_OUTPUT: &str = "StandardOutputAndStandardError.txt";

/// The line swift-testing writes when an expectation fails.
const ISSUE_MARKER: &str = "recorded an issue at";

/// What the tests reported, taken from the runner's output in the result
/// bundle.
///
/// The bundle is written incrementally into a `Staging` directory and only
/// sealed at the end of a run, so `xcresulttool` cannot open one belonging to a
/// run that was stopped at its first failure — which is every run outside CI.
/// The staged runner output is plain text and is there either way, and it holds
/// what swift-testing printed: the failed expression, and the comment the
/// author wrote under it.
///
/// Nothing in the test target has to cooperate.
/// A `#expect`, a `#require`, an `Issue.record` and an `XCTest` assertion all
/// arrive the same way, which is the property a convention nobody has to
/// remember gives you.
pub(super) fn collect_staged_issues(root: &Utf8Path) -> Option<String> {
    let mut issues = Vec::new();

    for file in staged_runner_output(&root.join(RESULT_BUNDLE)) {
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };

        issues.extend(issue_lines(&text));
    }

    if issues.is_empty() {
        return None;
    }

    Some(format!(
        "\n\nWhat the tests reported:\n\n```\n{}\n```\n",
        issues.join("\n")
    ))
}

/// The failed expectations in one runner log, each with the comment under it.
///
/// A failure is one line naming where it happened and what did not hold, then
/// any number of lines carrying the author's comment.
/// The comment lines are recognized by what they are not: the runner prefixes
/// its own activity with a timestamp, and marks suites and tests with a glyph.
fn issue_lines(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut in_issue = false;

    for line in text.lines() {
        let trimmed = line.trim();

        if trimmed.contains(ISSUE_MARKER) {
            found.push(strip(trimmed));
            in_issue = true;
            continue;
        }

        if !in_issue {
            continue;
        }

        // The runner's own activity resumes, so the comment has ended.
        if trimmed.is_empty() || trimmed.starts_with("t =") {
            in_issue = false;
            continue;
        }

        let comment = strip(trimmed);
        // Source lines the runner echoes back are already in the message above.
        if comment.starts_with("//") {
            continue;
        }

        found.push(format!("    {comment}"));
    }

    found
}

/// Every staged runner log inside a result bundle.
///
/// The path holds two UUIDs the run picks, so the tree is walked rather than
/// spelled out.
fn staged_runner_output(bundle: &Utf8Path) -> Vec<Utf8PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![bundle.join("Staging")];

    while let Some(directory) = stack.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };

        for entry in entries.flatten() {
            let Ok(path) = Utf8PathBuf::from_path_buf(entry.path()) else {
                continue;
            };

            if path.is_dir() {
                stack.push(path);
            } else if path.file_name() == Some(RUNNER_OUTPUT) {
                found.push(path);
            }
        }
    }

    found
}

/// The result bundle `xcodebuild` wrote, as `xcresulttool` reports it.
///
/// Apple's own reader for Apple's own format, so a failing `#expect`, a
/// `#require`, an `Issue.record` and an `XCTest` assertion all arrive the same
/// way, with the comment the author wrote.
/// Nothing in the test target has to know about this, which is the point: a
/// helper the suite must remember to call is a helper a future test will
/// forget.
///
/// Fetched once and read twice, for what failed and for what ran.
///
/// Returns `None` when there is no readable bundle.
/// A run stopped at its first failure is killed rather than asked to stop, so
/// the bundle it was part-way through writing may not be finished — which is
/// why the caller keeps a fallback rather than relying on this alone.
pub(super) fn bundle_document<R: ProcessRunner>(ctx: &Context, runner: &R) -> Option<Value> {
    let bundle = ctx.root.join(RESULT_BUNDLE);
    if !bundle.exists() {
        return None;
    }

    let output = runner
        .run(
            "xcrun",
            &[
                "xcresulttool",
                "get",
                "test-results",
                "tests",
                "--path",
                bundle.as_str(),
                "--compact",
            ],
            &ctx.root,
        )
        .ok()?;

    serde_json::from_str(&output.stdout).ok()
}

/// Every test the bundle records as having run, named the way a caller names
/// one.
///
/// A `Test Case` node's `nodeIdentifier` is its suite path and function —
/// `UISuite/ConversationListTests/clickSelects()` — which is the selector this
/// tool takes, minus the bundle prefix it adds itself.
/// So the two compare directly.
///
/// Empty when the document says nothing this recognizes, which the caller must
/// read as "cannot tell" rather than "nothing ran": `nodeIdentifier` is
/// optional in the schema, and treating its absence as a test that did not run
/// would fail a passing suite.
pub(super) fn executed_tests(document: &Value) -> Vec<String> {
    let mut found = Vec::new();
    walk_executed(document, &mut found);
    found.sort();
    found.dedup();
    found
}

fn walk_executed(value: &Value, found: &mut Vec<String>) {
    if let Value::Array(items) = value {
        for item in items {
            walk_executed(item, found);
        }
        return;
    }

    let Some(object) = value.as_object() else {
        return;
    };

    if object.get("nodeType").and_then(Value::as_str) == Some("Test Case")
        && let Some(id) = object.get("nodeIdentifier").and_then(Value::as_str)
    {
        found.push(id.to_owned());
    }

    for child in object.values() {
        walk_executed(child, found);
    }
}

/// The failures the bundle records, if any.
pub(super) fn bundle_issues(document: &Value) -> Option<String> {
    let mut failures = Vec::new();
    walk_failures(document, &mut Vec::new(), &mut failures);

    if failures.is_empty() {
        return None;
    }

    Some(format!(
        "\n\nWhat the tests reported, from {RESULT_BUNDLE}:\n\n```\n{}\n```\n",
        failures.join("\n")
    ))
}

/// Collect every failure message in the tests tree, under the test that holds
/// it.
///
/// The JSON is walked for the shapes it is known to use rather than
/// deserialized into the document's schema.
/// `xcresulttool` versions its output and has changed it between Xcode
/// releases; a walk that finds nothing degrades to a less helpful message,
/// where a failed parse would replace the failure being reported with a
/// complaint about reading it.
fn walk_failures(value: &Value, path: &mut Vec<String>, found: &mut Vec<String>) {
    if let Value::Array(items) = value {
        for item in items {
            walk_failures(item, path, found);
        }
        return;
    }

    let Some(object) = value.as_object() else {
        return;
    };

    let kind = object.get("nodeType").and_then(Value::as_str).unwrap_or("");
    let name = object.get("name").and_then(Value::as_str).unwrap_or("");

    // A failure is a node whose type says so, and whose name is the message.
    if kind.contains("Failure") && !name.is_empty() {
        let where_ = path.last().map_or("", String::as_str);
        found.push(format!("{where_}: {name}"));
    }

    // Test cases name themselves, so the innermost one seen above a failure is
    // the test that recorded it.
    let named = kind == "Test Case" && !name.is_empty();
    if named {
        path.push(name.to_owned());
    }

    for child in object.values() {
        walk_failures(child, path, found);
    }

    if named {
        path.pop();
    }
}

/// The file the UI tests write the process ids of the apps they launched to.
const APP_PIDS: &str = "app.pids";

/// Close the apps a stopped run left running.
///
/// Stopping a run kills `xcodebuild`, and that does not reach the app under
/// test: `testmanagerd` launched it, so it survives and sits on the screen
/// until somebody quits it.
///
/// By process id, written by each app itself, and never by name or bundle
/// identifier — the developer's own copy of JP has both of those, and closing
/// their window because a test failed would be a poor trade for a tidy screen.
/// A `TERM` rather than a kill, so the app puts itself away as it would on
/// quit.
pub(super) fn close_leftover_apps() {
    let Some(source) = screenshot_source() else {
        return;
    };

    let Ok(pids) = fs::read_to_string(source.join(APP_PIDS)) else {
        return;
    };

    for pid in pids.lines().map(str::trim).filter(|pid| !pid.is_empty()) {
        // Most of these are already gone: a test that finished terminated its
        // own app. Signalling a process that is not there fails, which is the
        // answer wanted anyway.
        let _signalled = std::process::Command::new("kill")
            .args(["-TERM", pid])
            .status();
    }

    let _removed = fs::remove_file(source.join(APP_PIDS));
}

/// What the UI tests recorded about their own failures.
///
/// Returns the empty string when they recorded nothing.
///
/// This exists because `xcodebuild` prints the header of a swift-testing issue
/// and drops the message under it, so a failing run arrives as a column of
/// identical `Issue recorded` lines.
/// The tests write the messages themselves, and this is where they are read
/// back.
pub(super) fn collect_failures() -> String {
    let Some(source) = screenshot_source() else {
        return "\n\nThe tests' own failure messages could not be looked for: HOME is unset, so \
                the runner's container has no derivable path.\n"
            .to_owned();
    };

    let path = source.join(FAILURE_LOG);
    let log = match fs::read_to_string(&path) {
        Ok(log) if !log.trim().is_empty() => log,

        // Read and empty, or not there at all. Both mean the same thing to a
        // reader and neither may be reported as silence: a run that failed while
        // writing nothing here looks, without this, exactly like a run that
        // failed for no stated reason.
        _ => {
            return format!(
                "\n\nThe tests recorded no failure messages ({path} is empty or absent). A \
                 failing `#expect` or `#require` writes nothing here on its own \u{2014} only the \
                 helpers that call `AppUnderTest.record` do. Read the assertion at the reported \
                 line, or route it through `expectAppears`, `expectTranscript` or `record` so the \
                 next run says what it saw.\n"
            );
        }
    };

    format!(
        "\n\nWhat the tests recorded:\n\n```\n{}\n```\n",
        log.trim_end()
    )
}

/// Copy this run's screenshots into the checkout and name them.
///
/// Returns the empty string when there are none, so a failure with nothing to
/// show reads exactly as it did before.
///
/// Copied rather than linked to: the container directory is emptied at the
/// start of every run, and a path that stops resolving the moment the next run
/// starts is worse than no path.
pub(super) fn collect_screenshots(root: &Utf8Path) -> String {
    let Some(source) = screenshot_source() else {
        return String::new();
    };

    let target = root.join(SCREENSHOT_DIR);
    let mut collected = Vec::new();

    let Ok(entries) = fs::read_dir(&source) else {
        return String::new();
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !Utf8Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
        {
            continue;
        }

        if fs::create_dir_all(&target).is_err() {
            return String::new();
        }

        if fs::copy(entry.path(), target.join(name)).is_ok() {
            collected.push(format!("{SCREENSHOT_DIR}/{name}"));
        }
    }

    if collected.is_empty() {
        return String::new();
    }

    collected.sort();
    format!(
        "\n\nWhat was on screen when the assertions failed. A tool result is text, so attach one \
         to see it:\n\n{}\n",
        collected
            .iter()
            .map(|path| format!("- `{path}`"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// Markers of a run that died rather than one that failed.
///
/// A process that crashes reports no failing test and no summary, so without
/// these the only honest thing left to say is "something went wrong, here is
/// the log".
/// `xcodebuild` announces a dead test runner; the rest are what the runtime
/// prints on its way out.
const CRASH_MARKERS: &[&str] = &[
    "Restarting after unexpected exit",
    "Fatal error",
    "Crashed:",
    "EXC_BAD_ACCESS",
    "Test runner exited",
];

/// A test run's output, ready to be searched.
///
/// Both runners split their output across both streams, so both are kept.
struct Log {
    stdout: String,
    stderr: String,
}

impl Log {
    fn from(output: &ProcessOutput) -> Self {
        Self {
            stdout: strip_ansi_escapes::strip_str(&output.stdout),
            stderr: strip_ansi_escapes::strip_str(&output.stderr),
        }
    }

    /// Both streams, one line at a time, exactly as written.
    fn raw_lines(&self) -> impl Iterator<Item = &str> {
        self.stdout.lines().chain(self.stderr.lines())
    }

    fn lines(&self) -> impl Iterator<Item = &str> {
        self.raw_lines().map(clean)
    }

    /// The run summary, naming how many tests ran.
    ///
    /// Empty when nothing ran, which is a distinct outcome from a passing run.
    fn summary(&self) -> String {
        strip(
            &self
                .lines()
                .filter(|line| is_run_summary(line))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    /// The lines saying the run died rather than reported a failure.
    fn crashes(&self) -> String {
        strip(
            &self
                .lines()
                .filter(|line| CRASH_MARKERS.iter().any(|marker| line.contains(marker)))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    /// The last `count` non-empty lines of both streams.
    ///
    /// The end rather than the beginning: a run with no diagnostic to show got
    /// as far as it got, and the head of the log is the build starting up.
    fn tail(&self, count: usize) -> String {
        let lines: Vec<&str> = self.lines().filter(|line| !line.is_empty()).collect();
        let start = lines.len().saturating_sub(count);

        strip(&lines[start..].join("\n"))
    }

    /// The lines naming what went wrong.
    ///
    /// Two spellings of a compiler error, because the two runners differ:
    /// `xcodebuild` prefixes the file and line, and `swift test` reports a
    /// driver-level failure such as a missing module with nothing in front of
    /// it.
    ///
    /// Continuation lines come along with the failure they belong to.
    /// swift-testing puts an issue's own message on a line of its own, under a
    /// header that names only the *kind* of issue, so a filter that kept the
    /// header alone would report every recorded issue as "Issue recorded" and
    /// throw away the sentence explaining it.
    fn failures(&self) -> String {
        strip(
            &self
                .raw_lines()
                .filter(|line| {
                    let cleaned = clean(line);
                    line.contains(ISSUE_DETAIL_MARKER)
                        || cleaned.contains("recorded an issue")
                        || cleaned.contains(": error:")
                        || cleaned.starts_with("error:")
                })
                .map(clean)
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}

/// The character swift-testing indents an issue's message with.
const ISSUE_DETAIL_MARKER: char = '\u{21b3}';

/// A log line with its decoration removed.
///
/// swift-testing prefixes each line with an SF Symbol from a private use area,
/// which is a glyph in Xcode and mojibake anywhere else.
fn clean(line: &str) -> &str {
    line.trim_start_matches(|c: char| !c.is_ascii()).trim()
}

/// Whether a line summarizes a run that executed at least one test.
///
/// A zero count is not a summary but the absence of one: a bundle whose tests
/// are all swift-testing always draws an `Executed 0 tests` line from the
/// `XCTest` runner, and reporting that as a pass would hide every mistyped
/// filter.
fn is_run_summary(line: &str) -> bool {
    // swift-testing: "Test run with 30 tests in 6 suites passed after 0.028s."
    if line.contains("Test run with") {
        return !line.contains("with 0 tests");
    }

    // XCTest: "Executed 20 tests, with 0 failures (0 unexpected) in 0.1 seconds"
    line.contains("Executed ") && !line.contains("Executed 0 tests")
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
