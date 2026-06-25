use std::collections::BTreeSet;

use jp_tool::Context;

use crate::util::{
    ToolResult, error,
    runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
};

pub(crate) async fn cargo_format(ctx: &Context, package: Option<String>) -> ToolResult {
    cargo_format_impl(ctx, package.as_deref(), &DuctProcessRunner)
}

fn cargo_format_impl<R: ProcessRunner>(
    ctx: &Context,
    package: Option<&str>,
    runner: &R,
) -> ToolResult {
    // 1. Run rustfmt over the workspace (or selected package).
    let cargo_scope = package.map_or("--all".to_owned(), |v| format!("--package={v}"));

    let ProcessOutput {
        stderr: cargo_stderr,
        status: cargo_status,
        stdout: cargo_stdout,
    } = runner.run_with_env(
        "cargo",
        &[
            "fmt",
            &cargo_scope,
            "--",
            "--color=never",
            "--files-with-diff",
        ],
        &ctx.root,
        // Prevent warnings from being treated as errors, e.g. on CI.
        &[("RUSTFLAGS", "-W warnings")],
    )?;

    if !cargo_status.is_success() {
        return error(format!("cargo fmt failed: {cargo_stderr}"));
    }

    // 2. Run comfort to reflow doc-comment paragraphs. The doc-comment
    //    formatting is independent of rustfmt's whitespace rules, so order
    //    doesn't matter; running comfort second keeps rustfmt as the
    //    authoritative source for everything outside `///`/`//!`.
    //
    //    `--language rust` restricts discovery to `.rs` files; the markdown
    //    flags still apply, but only to the markdown inside doc comments, not
    //    to standalone markdown files (READMEs, docs) in the workspace.
    let mut comfort_args = vec![
        "--list-changed",
        "--format-markdown",
        "--reference-links",
        "--prune-reference-links",
        "--language",
        "rust",
    ];
    if let Some(pkg) = package {
        comfort_args.push("--package");
        comfort_args.push(pkg);
    } else {
        comfort_args.push("--workspace");
    }

    let ProcessOutput {
        stderr: comfort_stderr,
        status: comfort_status,
        stdout: comfort_stdout,
    } = runner.run_with_env("comfort", &comfort_args, &ctx.root, &[])?;

    if !comfort_status.is_success() {
        return error(format!("comfort failed: {comfort_stderr}"));
    }

    // 3. Merge the two file lists into a deduplicated, sorted set, then
    //    strip the workspace-root prefix for a tidy report.
    let strip_root = |line: &str| -> String {
        line.trim_start_matches(ctx.root.as_str())
            .trim_start_matches('/')
            .to_owned()
    };

    let mut files: BTreeSet<String> = BTreeSet::new();
    for line in cargo_stdout.lines().chain(comfort_stdout.lines()) {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            files.insert(strip_root(trimmed));
        }
    }

    if files.is_empty() {
        Ok("No files to format.".into())
    } else {
        let listing = files.into_iter().collect::<Vec<_>>().join("\n- ");
        Ok(format!("Formatted files:\n- {listing}").into())
    }
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
