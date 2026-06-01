use std::collections::BTreeSet;

use jp_tool::Context;

use crate::util::{
    ToolResult, error,
    runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
};

pub(crate) async fn cargo_check(ctx: &Context, package: Option<String>) -> ToolResult {
    cargo_check_impl(ctx, package.as_deref(), &DuctProcessRunner)
}

fn cargo_check_impl<R: ProcessRunner>(
    ctx: &Context,
    package: Option<&str>,
    runner: &R,
) -> ToolResult {
    let clippy_scope = package.map_or("--workspace".to_owned(), |v| format!("--package={v}"));

    let ProcessOutput { stderr, status, .. } = runner.run_with_env(
        "cargo",
        &[
            "clippy",
            "--color=never",
            &clippy_scope,
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
    let clippy = strip_ansi_escapes::strip_str(stderr);
    let clippy = clippy.trim();

    let comfort_note = match comfort_check(ctx, package, runner)? {
        ComfortCheck::Clean => None,
        ComfortCheck::Drift(note) => Some(note),
        ComfortCheck::Failed(stderr) => return error(format!("comfort failed: {stderr}")),
    };

    let clippy_section = if clippy.is_empty() {
        "Check succeeded. No warnings or errors found.".to_owned()
    } else {
        format!("```\n{clippy}\n```\n")
    };

    match comfort_note {
        Some(note) => Ok(format!("{}\n\n{note}", clippy_section.trim_end()).into()),
        None => Ok(clippy_section.into()),
    }
}

enum ComfortCheck {
    /// All doc comments are well-formatted.
    Clean,
    /// Some doc comments would be reformatted; carries the user-facing note
    /// listing the offending files.
    Drift(String),
    /// comfort itself failed (parse error, bad package name); carries stderr.
    Failed(String),
}

/// Run comfort in `--check` mode to surface badly formatted doc comments.
///
/// Drift is not a failure: `cargo_fmt` auto-fixes it, so it comes back as a
/// [`ComfortCheck::Drift`] note rather than an error.
fn comfort_check<R: ProcessRunner>(
    ctx: &Context,
    package: Option<&str>,
    runner: &R,
) -> Result<ComfortCheck, std::io::Error> {
    let mut comfort_args = vec![
        "--check",
        "--list-changed",
        "--format-markdown",
        "--reference-links",
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
        stderr,
        status,
        stdout,
    } = runner.run_with_env("comfort", &comfort_args, &ctx.root, &[])?;

    let strip_root = |line: &str| -> String {
        line.trim_start_matches(ctx.root.as_str())
            .trim_start_matches('/')
            .to_owned()
    };

    let files: BTreeSet<String> = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(strip_root)
        .collect();

    if files.is_empty() {
        // In `--check` mode comfort exits non-zero with the drifting files on
        // stdout. A non-zero exit with no files listed is a genuine failure.
        if status.is_success() {
            return Ok(ComfortCheck::Clean);
        }
        return Ok(ComfortCheck::Failed(stderr));
    }

    let listing = files.into_iter().collect::<Vec<_>>().join("\n- ");
    Ok(ComfortCheck::Drift(format!(
        "Doc comments in the following files are badly formatted. Run `cargo_fmt` to auto-fix \
         them:\n- {listing}"
    )))
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
