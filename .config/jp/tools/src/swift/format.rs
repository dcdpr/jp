use jp_tool::Context;

use super::{SOURCE_PATHS, strip};
use crate::util::{
    ToolResult, error,
    runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
};

pub(crate) async fn swift_format(ctx: &Context, check: bool) -> ToolResult {
    swift_format_impl(ctx, check, &DuctProcessRunner)
}

fn swift_format_impl<R: ProcessRunner>(ctx: &Context, check: bool, runner: &R) -> ToolResult {
    // `swift format lint` reports rule violations without rewriting; the default
    // subcommand rewrites but does not lint. Both read `apps/macos/.swift-format`.
    let mut args = if check {
        vec!["format", "lint", "--strict"]
    } else {
        vec!["format", "--in-place"]
    };
    args.extend(["--recursive", "--parallel"]);
    args.extend(SOURCE_PATHS);

    // `swift format` reports on stderr; stdout carries rewritten source only when
    // writing to a pipe, which `--in-place` and `lint` never do.
    let ProcessOutput { stderr, status, .. } = runner.run("swift", &args, &ctx.root)?;

    if check {
        let findings = strip(&stderr);
        return if status.is_success() && findings.is_empty() {
            Ok("Swift sources are correctly formatted.".to_owned().into())
        } else {
            error(format!(
                "Swift formatting or lint violations:\n\n```\n{findings}\n```"
            ))
        };
    }

    if !status.is_success() {
        return error(format!("swift format failed: {}", strip(&stderr)));
    }

    // `--in-place` is silent: it neither lists what it rewrote nor says that it
    // rewrote nothing. So this reports what was formatted, and claims nothing
    // about what changed — `swift_format` with `check` set is what answers that.
    let diagnostics = strip(&stderr);
    Ok(if diagnostics.is_empty() {
        format!("Formatted {}.", SOURCE_PATHS.join(", ")).into()
    } else {
        format!(
            "Formatted {}.\n\n```\n{diagnostics}\n```",
            SOURCE_PATHS.join(", ")
        )
        .into()
    })
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
