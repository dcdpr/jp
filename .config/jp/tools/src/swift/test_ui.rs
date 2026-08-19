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
        RESULT_BUNDLE, UI_BUNDLE, clear_result_bundle, clear_screenshots, close_leftover_apps,
        collect_bundle_issues, collect_failures, collect_screenshots, collect_staged_issues,
        outcome, ui_bundle_filter,
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
    std::env::var("CI").is_ok_and(|value| !value.is_empty())
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
        match enumerate(ctx, runner) {
            Ok(names) if !names.is_empty() => {
                message.push_str("\n\nWhat there is to run:\n\n```\n");
                message.push_str(&names.join("\n"));
                message.push_str("\n```\n");
            }
            _ => message.push_str(
                "\n\nA name is a whole type path with a swift-testing function's trailing `()`, \
                 such as `UISuite/ConversationListTests/clickSelects()`.",
            ),
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
    let reported = collect_bundle_issues(ctx, runner)
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

    match outcome(&output, "JP") {
        Ok(summary) => Ok(format!("```\n{summary}\n```").into()),
        Err(message) => error(message + &detail),
    }
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
/// A name matching nothing fails the whole run rather than being passed over,
/// so a typo is reported instead of quietly narrowing the run to the rest.
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
