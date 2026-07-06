use jp_tool::Context;

use crate::util::{
    ToolResult,
    runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
};

pub(crate) async fn cargo_expand(
    ctx: &Context,
    item: String,
    package: Option<String>,
    checksum_freshness: bool,
) -> ToolResult {
    cargo_expand_impl(ctx, &item, package, checksum_freshness, &DuctProcessRunner)
}

fn cargo_expand_impl<R: ProcessRunner>(
    ctx: &Context,
    item: &str,
    package: Option<String>,
    checksum_freshness: bool,
    runner: &R,
) -> ToolResult {
    let package = package.map(|v| format!("--package={v}"));
    let mut args = vec!["--quiet", "expand", "--color=never"];
    if let Some(package) = package.as_deref() {
        args.push(package);
    }
    args.push(item);

    let mut env = vec![("RUST_BACKTRACE", "1")];
    if checksum_freshness {
        // Use content checksums instead of file mtimes for cargo's freshness
        // checks, so that sibling checkouts (git worktrees) sharing a target
        // dir cannot serve each other's stale artifacts. Matches CI. Requires
        // nightly cargo. See rust-lang/cargo#14136.
        env.push(("CARGO_UNSTABLE_CHECKSUM_FRESHNESS", "true"));
    }

    let ProcessOutput {
        stdout,
        stderr,
        status,
    } = runner.run_with_env("cargo", &args, &ctx.root, &env)?;

    if !status.is_success() {
        return Err(format!("Cargo command failed: {stderr}").into());
    }

    Ok(format!("```rust\n{}\n```\n", stdout.trim()).into())
}

#[cfg(test)]
#[path = "expand_tests.rs"]
mod tests;
