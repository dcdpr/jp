use jp_tool::Context;

use crate::util::{
    ToolResult, error,
    runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
};

pub(crate) async fn cargo_check(ctx: &Context, package: Option<String>) -> ToolResult {
    cargo_check_impl(ctx, package, &DuctProcessRunner)
}

fn cargo_check_impl<R: ProcessRunner>(
    ctx: &Context,
    package: Option<String>,
    runner: &R,
) -> ToolResult {
    let package = package.map_or("--workspace".to_owned(), |v| format!("--package={v}"));

    let ProcessOutput { stderr, status, .. } = runner.run_with_env(
        "cargo",
        &[
            "clippy",
            "--color=never",
            &package,
            "--quiet",
            "--all-targets",
        ],
        &ctx.root,
        // Prevent warnings from being treated as errors, e.g. on CI.
        &[("RUSTFLAGS", "-W warnings")],
    )?;

    if !status.is_success() {
        return error(format!("Cargo command failed: {stderr}"));
    }

    // Strip ANSI escape codes
    let content = strip_ansi_escapes::strip_str(stderr);
    let content = content.trim();

    if content.is_empty() {
        Ok("Check succeeded. No warnings or errors found.".into())
    } else {
        Ok(format!("```\n{content}\n```\n").into())
    }
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
