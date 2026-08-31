use camino::Utf8Path;

use super::MAX_DIAGNOSTIC_BYTES;
use crate::util::{
    ToolResult,
    runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
    truncate,
};

/// Cap for expanded source.
///
/// Larger than [`MAX_DIAGNOSTIC_BYTES`]: expanded macro output is the payload
/// the caller asked for, not incidental noise.
const MAX_EXPANDED_BYTES: usize = 100_000;

pub(crate) async fn cargo_expand(
    root: &Utf8Path,
    rustflags: &str,
    profile: Option<&str>,
    item: String,
    package: Option<String>,
    checksum_freshness: bool,
) -> ToolResult {
    cargo_expand_impl(
        root,
        rustflags,
        profile,
        &item,
        package,
        checksum_freshness,
        &DuctProcessRunner,
    )
}

fn cargo_expand_impl<R: ProcessRunner>(
    root: &Utf8Path,
    rustflags: &str,
    profile: Option<&str>,
    item: &str,
    package: Option<String>,
    checksum_freshness: bool,
    runner: &R,
) -> ToolResult {
    let package = package.map(|v| format!("--package={v}"));
    let profile_arg = profile.map(|name| format!("--profile={name}"));
    let mut args = vec!["--quiet", "expand", "--color=never"];
    if let Some(package) = package.as_deref() {
        args.push(package);
    }
    if let Some(profile) = profile_arg.as_deref() {
        args.push(profile);
    }
    args.push(item);

    let mut env = vec![("RUST_BACKTRACE", "1"), ("RUSTFLAGS", rustflags)];
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
    } = runner.run_with_env("cargo", &args, root, &env)?;

    if !status.is_success() {
        return Err(format!(
            "Cargo command failed: {}",
            truncate(&stderr, MAX_DIAGNOSTIC_BYTES)
        )
        .into());
    }

    Ok(format!(
        "```rust\n{}\n```\n",
        truncate(stdout.trim(), MAX_EXPANDED_BYTES)
    )
    .into())
}

#[cfg(test)]
#[path = "expand_tests.rs"]
mod tests;
