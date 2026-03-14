use jp_tool::Context;

use crate::util::{
    ToolResult,
    runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
};

pub(crate) async fn cargo_expand(
    ctx: &Context,
    item: String,
    package: Option<String>,
) -> ToolResult {
    cargo_expand_impl(ctx, &item, package, &DuctProcessRunner)
}

fn cargo_expand_impl<R: ProcessRunner>(
    ctx: &Context,
    item: &str,
    package: Option<String>,
    runner: &R,
) -> ToolResult {
    let package = package.map(|v| format!("--package={v}"));
    let mut args = vec!["--quiet", "expand", "--color=never"];
    if let Some(package) = package.as_deref() {
        args.push(package);
    }
    args.push(item);

    let ProcessOutput {
        stdout,
        stderr,
        status,
    } = runner.run_with_env("cargo", &args, &ctx.root, &[("RUST_BACKTRACE", "1")])?;

    if !status.is_success() {
        return Err(format!("Cargo command failed: {stderr}").into());
    }

    Ok(format!("```rust\n{}\n```\n", stdout.trim()).into())
}

#[cfg(test)]
#[path = "expand_tests.rs"]
mod tests;
