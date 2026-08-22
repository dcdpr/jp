use jp_tool::Context;

use super::{
    PROJECT_PATH, SCHEME, prepare,
    report::{outcome, unit_bundle_filter},
};
use crate::util::{
    ToolResult, error,
    runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
};

/// The `SwiftPM` package holding the accessibility driver.
const DRIVE_PACKAGE: &str = "apps/macos/Tools/jpdrive";

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

pub(crate) async fn swift_test(
    ctx: &Context,
    testname: Option<String>,
    target: Option<String>,
) -> ToolResult {
    swift_test_impl(
        ctx,
        testname.as_deref(),
        target.as_deref(),
        &DuctProcessRunner,
    )
}

fn swift_test_impl<R: ProcessRunner>(
    ctx: &Context,
    testname: Option<&str>,
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
        let output = run_drive(ctx, testname, runner)?;
        match outcome(&output, "DriveKit") {
            Ok(summary) => summaries.push(summary),
            Err(message) => return error(message),
        }
    }

    if target.includes_app() {
        // Tests run against the Debug configuration, so the library they link is
        // the one in cargo's `debug` directory.
        if let Some(failure) = prepare(ctx, "debug", runner)? {
            return error(failure);
        }

        let output = run_app(ctx, testname, runner)?;
        match outcome(&output, "JP") {
            Ok(summary) => summaries.push(summary),
            Err(message) => return error(message),
        }
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

/// Run the app's unit-test bundle.
///
/// The filter always names the bundle, even with no test to narrow to: the UI
/// bundle is in the same scheme, and a run that named no bundle would launch
/// the app once per UI test.
fn run_app<R: ProcessRunner>(
    ctx: &Context,
    testname: Option<&str>,
    runner: &R,
) -> Result<ProcessOutput, std::io::Error> {
    let filter = unit_bundle_filter(testname);

    // `-quiet` is deliberately absent. It suppresses the test summary along with
    // everything else, which makes a filter that matched nothing look exactly
    // like a passing run.
    let args = vec![
        "test",
        "-project",
        PROJECT_PATH,
        "-scheme",
        SCHEME,
        "-destination",
        "platform=macOS",
        &filter,
    ];

    runner.run("xcodebuild", &args, &ctx.root)
}

#[cfg(test)]
#[path = "test_tests.rs"]
mod tests;
