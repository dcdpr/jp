use jp_tool::Context;

use super::{PROJECT_PATH, SCHEME, prepare, report, strip};
use crate::util::{
    ToolResult, error,
    runner::{DuctProcessRunner, ProcessRunner},
};

pub(crate) async fn swift_check(ctx: &Context, configuration: Option<String>) -> ToolResult {
    swift_check_impl(ctx, configuration.as_deref(), &DuctProcessRunner)
}

fn swift_check_impl<R: ProcessRunner>(
    ctx: &Context,
    configuration: Option<&str>,
    runner: &R,
) -> ToolResult {
    let configuration = configuration.unwrap_or("Debug");
    // Cargo's profile directory for a configuration, matching `CARGO_PROFILE` in
    // the Xcode project.
    let profile = if configuration == "Release" {
        "release"
    } else {
        "debug"
    };

    if let Some(failure) = prepare(ctx, profile, runner)? {
        return error(failure);
    }

    let output = runner.run(
        "xcodebuild",
        &[
            "build",
            "-project",
            PROJECT_PATH,
            "-scheme",
            SCHEME,
            "-configuration",
            configuration,
            "-destination",
            "platform=macOS",
            // Nothing is run or installed, so skip signing and its keychain
            // prompts.
            "CODE_SIGNING_ALLOWED=NO",
            "-quiet",
        ],
        &ctx.root,
    )?;

    let diagnostics = strip(&output.stdout);

    if output.status.is_success() {
        return Ok(if diagnostics.is_empty() {
            "Build succeeded. No warnings or errors found."
                .to_owned()
                .into()
        } else {
            format!("```\n{diagnostics}\n```\n").into()
        });
    }

    error(format!(
        "Swift build failed:\n\n```\n{}\n```",
        report(&output, "xcodebuild")
    ))
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
