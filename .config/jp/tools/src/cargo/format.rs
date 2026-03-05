use jp_tool::Context;

use crate::util::{
    ToolResult, error,
    runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
};

pub(crate) async fn cargo_format(ctx: &Context, package: Option<String>) -> ToolResult {
    cargo_format_impl(ctx, package, &DuctProcessRunner)
}

fn cargo_format_impl<R: ProcessRunner>(
    ctx: &Context,
    package: Option<String>,
    runner: &R,
) -> ToolResult {
    let package = package.map_or("--all".to_owned(), |v| format!("--package={v}"));

    let ProcessOutput {
        stderr,
        status,
        stdout,
    } = runner.run_with_env(
        "cargo",
        &["fmt", &package, "--", "--color=never", "--files-with-diff"],
        &ctx.root,
        // Prevent warnings from being treated as errors, e.g. on CI.
        &[("RUSTFLAGS", "-W warnings")],
    )?;

    if !status.is_success() {
        return error(format!("Cargo command failed: {stderr}"));
    }

    if stdout.trim().is_empty() {
        Ok("No files to format.".into())
    } else {
        let files = stdout
            .trim()
            .lines()
            .map(|line| line.trim_start_matches(ctx.root.as_str()))
            .map(|line| line.trim_start_matches('/'))
            .collect::<Vec<_>>()
            .join("\n- ");

        Ok(format!("Formatted files:\n- {files}").into())
    }
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
