use jp_process::{ProcessOutput, ProcessRunner, SystemProcessRunner};
use jp_tool::Context;

use super::{
    PROJECT_PATH, SCHEME, prepare,
    report::{
        bundle_document, clear_result_bundle, executed_tests, outcome, unit_bundle_filter,
        unmatched,
    },
};
use crate::util::{ToolResult, error};

/// The `SwiftPM` package holding the accessibility driver.
const DRIVE_PACKAGE: &str = "apps/macos/Tools/jpdrive";

/// Where `xcodebuild` writes the result bundle for an app run naming more than
/// one test, relative to the repository root.
///
/// Apart from the UI tests' bundle, so a unit run never deletes the bundle a UI
/// run is reading.
const RESULT_BUNDLE: &str = "tmp/swift-test/run.xcresult";

/// Which test suites to run.
///
/// The two are built and filtered by different tools, so a run has to name
/// which it means rather than passing one filter to both: a suite name that
/// addresses the app matches nothing in the package, and a run that matched
/// nothing is reported as a failure.
///
/// The UI tests are not reachable from here at all.
/// They launch the app and take the screen for as long as they run, so they
/// belong to `swift_test_ui`, which has to be asked for them by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    /// The app's unit-test bundle, through `xcodebuild`.
    App,
    /// The driver package's tests, through `swift test`.
    Drive,
    /// Both.
    All,
}

impl Target {
    fn parse(value: Option<&str>) -> Result<Self, String> {
        match value {
            None | Some("all") => Ok(Self::All),
            Some("app") => Ok(Self::App),
            Some("drive") => Ok(Self::Drive),
            Some("ui") => Err(
                "the UI tests run through `swift_test_ui`, which takes the tests to run by name. \
                 They drive the app through the screen, so a run costs about six seconds per test \
                 and takes over the display."
                    .to_owned(),
            ),
            Some(other) => Err(format!(
                "unknown target '{other}', expected one of: app, drive, all"
            )),
        }
    }

    fn includes_app(self) -> bool {
        self != Self::Drive
    }

    fn includes_drive(self) -> bool {
        self != Self::App
    }
}

/// Run the fast test suites, narrowed to `testnames` when it is not empty.
pub(crate) async fn swift_test(
    ctx: &Context,
    testnames: &[String],
    target: Option<String>,
) -> ToolResult {
    swift_test_impl(ctx, testnames, target.as_deref(), &SystemProcessRunner)
}

fn swift_test_impl<R: ProcessRunner>(
    ctx: &Context,
    testnames: &[String],
    target: Option<&str>,
    runner: &R,
) -> ToolResult {
    let target = match Target::parse(target) {
        Ok(target) => target,
        Err(message) => return error(message),
    };

    let mut summaries = Vec::new();

    // The driver package needs no Xcode project and runs in milliseconds, so it
    // goes first: a failure there is reported before a minute of `xcodebuild`.
    if target.includes_drive() {
        // One `swift test` per name. A run with several filters reports one
        // summary for all of them, so a name matching nothing would go
        // unnoticed beside one that matched; separate runs each have to report
        // tests of their own. Past the first, a run rebuilds nothing.
        let filters: Vec<Option<&str>> = if testnames.is_empty() {
            vec![None]
        } else {
            testnames.iter().map(|name| Some(name.as_str())).collect()
        };

        for filter in filters {
            let label = match filter {
                Some(name) if testnames.len() > 1 => format!("DriveKit `{name}`"),
                _ => "DriveKit".to_owned(),
            };

            let output = run_drive(ctx, filter, runner)?;
            match outcome(&output, &label) {
                Ok(summary) => summaries.push(summary),
                Err(message) => return error(message),
            }
        }
    }

    if target.includes_app() {
        // Tests run against the Debug configuration, so the library they link is
        // the one in cargo's `debug` directory.
        if let Some(failure) = prepare(ctx, "debug", runner)? {
            return error(failure);
        }

        // A single name that matches nothing leaves no summary, which `outcome`
        // reports. With several, `xcodebuild` unions the selectors and passes
        // over one that matches nothing, so only the result bundle can say
        // which names ran.
        let check_names = testnames.len() > 1;
        if check_names {
            clear_result_bundle(&ctx.root, RESULT_BUNDLE);
        }

        let output = run_app(ctx, testnames, check_names, runner)?;
        let summary = match outcome(&output, "JP") {
            Ok(summary) => summary,
            Err(message) => return error(message),
        };

        if check_names {
            let executed = bundle_document(ctx, RESULT_BUNDLE, runner)
                .as_ref()
                .map(executed_tests)
                .unwrap_or_default();
            let missed = unmatched(testnames, &executed);
            if !missed.is_empty() {
                return error(format!(
                    "These names matched no test in the app's unit tests, and `xcodebuild` passed \
                     over them rather than failing:\n\n```\n{}\n```\n\nWhat did \
                     run:\n\n```\n{summary}\n```",
                    missed.join("\n")
                ));
            }
        }

        summaries.push(summary);
    }

    Ok(format!("```\n{}\n```", summaries.join("\n")).into())
}

/// Run the driver package's tests.
fn run_drive<R: ProcessRunner>(
    ctx: &Context,
    testname: Option<&str>,
    runner: &R,
) -> Result<ProcessOutput, std::io::Error> {
    let mut args = vec!["test", "--package-path", DRIVE_PACKAGE];
    if let Some(testname) = testname {
        // `swift test` takes a regex over `Suite/test`, which is the same shape
        // callers already write for the app bundle.
        args.extend(["--filter", testname]);
    }

    runner.run("swift", &args, &ctx.root)
}

/// Run the app's unit-test bundle, narrowed to `testnames` when it is not
/// empty, with one `-only-testing` selector per name.
///
/// The filter always names the bundle, even with no test to narrow to: the UI
/// bundle is in the same scheme, and a run that named no bundle would launch
/// the app once per UI test.
///
/// With `result_bundle`, the run writes its result bundle to [`RESULT_BUNDLE`].
fn run_app<R: ProcessRunner>(
    ctx: &Context,
    testnames: &[String],
    result_bundle: bool,
    runner: &R,
) -> Result<ProcessOutput, std::io::Error> {
    let filters: Vec<String> = if testnames.is_empty() {
        vec![unit_bundle_filter(None)]
    } else {
        testnames
            .iter()
            .map(|name| unit_bundle_filter(Some(name)))
            .collect()
    };

    // `-quiet` is deliberately absent. It suppresses the test summary along with
    // everything else, which makes a filter that matched nothing look exactly
    // like a passing run.
    let mut args = vec![
        "test",
        "-project",
        PROJECT_PATH,
        "-scheme",
        SCHEME,
        "-destination",
        "platform=macOS",
    ];
    if result_bundle {
        args.extend(["-resultBundlePath", RESULT_BUNDLE]);
    }
    args.extend(filters.iter().map(String::as_str));

    runner.run("xcodebuild", &args, &ctx.root)
}

#[cfg(test)]
#[path = "test_tests.rs"]
mod tests;
