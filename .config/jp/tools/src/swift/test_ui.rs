//! `swift_test_ui` — run named UI tests against the macOS app.
//!
//! Separate from [`swift_test`] because a UI test is a different kind of thing
//! to run.
//! It launches the app, takes the screen for as long as it drives it, and costs
//! seconds rather than milliseconds.
//! Running the suite is a job for CI; running two tests you just wrote is a job
//! for this.
//!
//! Which is why the tests to run are required rather than optional.
//! There is no spelling of this tool that means "run all of them" — reaching
//! for that is how a red-green loop turns into a minute per iteration.
//!
//! [`swift_test`]: super::test

use jp_tool::Context;

use super::{
    PROJECT_PATH, SCHEME, prepare,
    report::{
        RESULT_BUNDLE, UI_BUNDLE, bundle_document, bundle_issues, clear_result_bundle,
        clear_screenshots, close_leftover_apps, collect_failures, collect_screenshots,
        collect_staged_issues, executed_tests, outcome, ui_bundle_filter,
    },
};
use crate::util::{
    ToolResult, error,
    runner::{DuctProcessRunner, ProcessOutput, ProcessRunner, Stopped},
};

/// The line `xcodebuild` prints when a swift-testing expectation fails.
///
/// Watched for rather than waited on: the run is stopped the moment one
/// appears, so a broken app costs one test's worth of time instead of the whole
/// suite's.
const FAILURE_MARKER: &str = "recorded an issue";

/// Whether this process is running under continuous integration.
///
/// CI wants every result from one run, because nobody is sitting there to run
/// it again; a person at a keyboard wants the first failure as fast as
/// possible.
/// Same suite, opposite priorities, so the environment decides.
fn under_ci() -> bool {
    is_ci(std::env::var("CI").ok().as_deref())
}

/// Whether `value` — the `CI` variable, or `None` when it is unset — means
/// CI.
///
/// Split out so the rule can be checked without writing to the process
/// environment.
/// `set_var` is unsafe because another thread may be reading, and `under_ci` is
/// reached by every test that runs the tool, so a test that set `CI` would be
/// racing them.
fn is_ci(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.is_empty())
}

pub(crate) async fn swift_test_ui(ctx: &Context, tests: Option<Vec<String>>) -> ToolResult {
    swift_test_ui_impl(ctx, tests.as_deref(), &DuctProcessRunner)
}

fn swift_test_ui_impl<R: ProcessRunner>(
    ctx: &Context,
    tests: Option<&[String]>,
    runner: &R,
) -> ToolResult {
    let Some(tests) = tests.filter(|tests| !tests.is_empty()) else {
        let mut message = "`tests` is required: name the suites or tests to run. Every one of \
                           these launches the app and drives it through the screen, so there is \
                           deliberately no way to ask for all of them; that is CI's job, through \
                           `just test-app-ui`."
            .to_owned();

        // Asked of the bundle rather than read off a list someone maintains:
        // a list in a document rots, and the one place that cannot is the
        // bundle itself.
        //
        // Prepared first because enumeration reads the generated Xcode project,
        // which is gitignored and produced by `prepare`. Reaching enumeration
        // before it leaves the list unavailable on a fresh checkout, which is
        // exactly where a caller most needs to be told what there is. A
        // preparation failure needs no reporting of its own: enumeration then
        // fails too, and the naming convention below covers both.
        let names = match prepare(ctx, "debug", runner) {
            Ok(None) => enumerate(ctx, runner).unwrap_or_default(),
            _ => Vec::new(),
        };

        if names.is_empty() {
            message.push_str(
                "\n\nA name is a whole type path with a swift-testing function's trailing `()`, \
                 such as `UISuite/ConversationListTests/clickSelects()`.",
            );
        } else {
            message.push_str("\n\nWhat there is to run:\n\n```\n");
            message.push_str(&names.join("\n"));
            message.push_str("\n```\n");
        }

        return error(message);
    };

    if let Some(failure) = prepare(ctx, "debug", runner)? {
        return error(failure);
    }

    // Emptied before the run so what is collected afterwards belongs to it and
    // not to the run before.
    clear_screenshots();
    clear_result_bundle(&ctx.root);

    let (output, stopped) = run(ctx, tests, runner)?;

    // What the runner itself printed, in the order the sources can be trusted.
    //
    // A sealed bundle is the richest, and only a run that finished has one. The
    // staged runner output covers the rest — every run stopped at its first
    // failure — and holds the same expectations and comments as plain text. The
    // log the tests keep themselves is last, and needed only for a failure
    // recorded before the runner wrote anything.
    let document = bundle_document(ctx, runner);
    let reported = document
        .as_ref()
        .and_then(bundle_issues)
        .or_else(|| collect_staged_issues(&ctx.root))
        .unwrap_or_else(collect_failures);
    let detail = reported + &collect_screenshots(&ctx.root);

    if stopped.is_yes() {
        close_leftover_apps();

        return error(format!(
            "Stopped the run at the first failure, so the tests after it did not run. Set `CI=1` \
             to let a run finish and report everything.{detail}"
        ));
    }

    let summary = match outcome(&output, "JP") {
        Ok(summary) => summary,
        Err(message) => return error(message + &detail),
    };

    // `xcodebuild` unions the `-only-testing` selectors and ignores any that
    // match nothing, so a run naming one real test and one typo passes with the
    // typo unmentioned. The bundle is the only record of what actually ran.
    let executed = document.as_ref().map(executed_tests).unwrap_or_default();
    let missed = unmatched(tests, &executed);
    if !missed.is_empty() {
        return error(format!(
            "These names matched no test, and `xcodebuild` passed over them rather than \
             failing:\n\n```\n{}\n```\n\nWhat did run:\n\n```\n{summary}\n```",
            missed.join("\n")
        ));
    }

    Ok(format!("```\n{summary}\n```").into())
}

/// The requested names that nothing in `executed` answers to.
///
/// A name is either a whole test (`Suite/test()`) or a suite standing for
/// everything beneath it, so a name is matched by an identifier that equals it
/// or continues it after a `/`.
///
/// Empty when `executed` is, which is deliberate: an unreadable bundle means
/// the question cannot be answered, and answering "none of them ran" would fail
/// a suite that passed.
fn unmatched(requested: &[String], executed: &[String]) -> Vec<String> {
    if executed.is_empty() {
        return Vec::new();
    }

    requested
        .iter()
        .filter(|name| {
            !executed.iter().any(|id| {
                id == *name
                    || id
                        .strip_prefix(name.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            })
        })
        .cloned()
        .collect()
}

/// Every test in the UI bundle, as `xcodebuild` reports them.
///
/// Costs a build, which is why it is only reached when a caller named nothing
/// and the run is not going to happen anyway.
///
/// The JSON is walked for `identifier` keys rather than deserialized into the
/// document's shape: this is an error path, the shape has changed between Xcode
/// releases, and a parse failure here should cost the caller a less helpful
/// message rather than a second failure on top of the first.
fn enumerate<R: ProcessRunner>(ctx: &Context, runner: &R) -> Result<Vec<String>, std::io::Error> {
    let output = runner.run(
        "xcodebuild",
        &[
            "test",
            "-project",
            PROJECT_PATH,
            "-scheme",
            SCHEME,
            "-destination",
            "platform=macOS",
            "-enumerate-tests",
            "-test-enumeration-style",
            "flat",
            "-test-enumeration-format",
            "json",
        ],
        &ctx.root,
    )?;

    let Some(start) = output.stdout.find('{') else {
        return Ok(Vec::new());
    };

    let Ok(json) = serde_json::from_str::<serde_json::Value>(&output.stdout[start..]) else {
        return Ok(Vec::new());
    };

    let mut identifiers = Vec::new();
    collect_identifiers(&json, &mut identifiers);

    // `-enumerate-tests` reports the whole scheme whatever `-only-testing`
    // says, so the unit bundle is in there too. The prefix comes off because
    // this tool puts it back on.
    let prefix = format!("{UI_BUNDLE}/");
    let mut names: Vec<String> = identifiers
        .iter()
        .filter_map(|id| id.strip_prefix(&prefix))
        .map(str::to_owned)
        .collect();

    names.sort();
    names.dedup();

    Ok(names)
}

/// Gather every `identifier` string anywhere in `value`.
fn collect_identifiers(value: &serde_json::Value, into: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                if key == "identifier"
                    && let Some(name) = child.as_str()
                {
                    into.push(name.to_owned());
                }
                collect_identifiers(child, into);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_identifiers(item, into);
            }
        }
        _ => {}
    }
}

/// Run the named tests, stopping at the first failure unless under CI.
///
/// One `-only-testing` argument each, which `xcodebuild` unions.
///
/// `xcodebuild` passes over a name matching nothing: it runs whichever
/// selectors do match and exits zero, so a call naming one real test and one
/// typo would report the real one as a pass and say nothing about the typo.
/// The caller compares the result bundle's executed tests against what was
/// asked for, the bundle being the only record of which it is.
///
/// Stopped from out here because nothing inside can do it. swift-testing has no
/// cancellation, and a test that exits its own process makes `xcodebuild`
/// relaunch the runner, finish the remaining tests, and report the whole thing
/// as passing.
fn run<R: ProcessRunner>(
    ctx: &Context,
    tests: &[String],
    runner: &R,
) -> Result<(ProcessOutput, Stopped), std::io::Error> {
    let filters: Vec<String> = tests.iter().map(|test| ui_bundle_filter(test)).collect();

    let mut args = vec![
        "test",
        "-project",
        PROJECT_PATH,
        "-scheme",
        SCHEME,
        "-destination",
        "platform=macOS",
        // Written where the report can read it back. Without this the bundle
        // lands in derived data under a timestamped name, which would have to be
        // scraped out of the log.
        "-resultBundlePath",
        RESULT_BUNDLE,
    ];
    args.extend(filters.iter().map(String::as_str));

    if under_ci() {
        return Ok((runner.run("xcodebuild", &args, &ctx.root)?, Stopped::No));
    }

    runner.run_until("xcodebuild", &args, &ctx.root, &|line| {
        line.contains(FAILURE_MARKER)
    })
}

#[cfg(test)]
#[path = "test_ui_tests.rs"]
mod tests;
