use std::collections::BTreeSet;

use camino::Utf8Path;

use super::MAX_DIAGNOSTIC_BYTES;
use crate::util::{
    ToolResult, error,
    runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
    truncate,
};

pub(crate) async fn cargo_check(
    root: &Utf8Path,
    profile: Option<&str>,
    package: Option<String>,
    checksum_freshness: bool,
) -> ToolResult {
    cargo_check_impl(
        root,
        profile,
        package.as_deref(),
        checksum_freshness,
        &DuctProcessRunner,
    )
}

fn cargo_check_impl<R: ProcessRunner>(
    root: &Utf8Path,
    profile: Option<&str>,
    package: Option<&str>,
    checksum_freshness: bool,
    runner: &R,
) -> ToolResult {
    let clippy_scope = package.map_or("--workspace".to_owned(), |v| format!("--package={v}"));
    let profile_arg = profile.map(|name| format!("--profile={name}"));

    // Prevent warnings from being treated as errors, e.g. on CI.
    let mut env = vec![("RUSTFLAGS", "-W warnings")];
    if checksum_freshness {
        // Use content checksums instead of file mtimes for cargo's freshness
        // checks, so that sibling checkouts (git worktrees) sharing a target
        // dir cannot serve each other's stale artifacts. Matches CI. Requires
        // nightly cargo. See rust-lang/cargo#14136.
        env.push(("CARGO_UNSTABLE_CHECKSUM_FRESHNESS", "true"));
    }

    let mut args = vec![
        "clippy",
        "--color=never",
        clippy_scope.as_str(),
        "--quiet",
        "--all-targets",
        // Matches `just lint-ci`. Code behind an optional feature is not
        // compiled without this, so its lints surface only on CI.
        "--all-features",
    ];
    if let Some(profile) = profile_arg.as_deref() {
        args.push(profile);
    }

    let ProcessOutput { stderr, status, .. } = runner.run_with_env("cargo", &args, root, &env)?;

    if !status.is_success() {
        return error(format!(
            "Cargo command failed: {}",
            truncate(&stderr, MAX_DIAGNOSTIC_BYTES)
        ));
    }

    // Strip ANSI escape codes
    let clippy = strip_ansi_escapes::strip_str(stderr);
    let clippy = truncate(clippy.trim(), MAX_DIAGNOSTIC_BYTES);

    let comfort_note = match comfort_check(root, package, runner)? {
        ComfortCheck::Clean => None,
        ComfortCheck::Drift(note) => Some(note),
        ComfortCheck::Failed(stderr) => {
            return error(format!(
                "comfort failed: {}",
                truncate(&stderr, MAX_DIAGNOSTIC_BYTES)
            ));
        }
    };

    let Some(note) = comfort_note else {
        return Ok(if clippy.is_empty() {
            "Check succeeded. No warnings or errors found."
                .to_owned()
                .into()
        } else {
            format!("```\n{clippy}\n```\n").into()
        });
    };

    // The header is scoped to what clippy alone found. A bare "Check succeeded"
    // would contradict the drift note that follows.
    let header = if clippy.is_empty() {
        "`cargo clippy` found no warnings or errors.".to_owned()
    } else {
        format!("```\n{clippy}\n```")
    };

    Ok(format!("{header}\n\n{note}").into())
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
    root: &Utf8Path,
    package: Option<&str>,
    runner: &R,
) -> Result<ComfortCheck, std::io::Error> {
    let mut comfort_args = vec![
        "--check",
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
        stderr,
        status,
        stdout,
    } = runner.run_with_env("comfort", &comfort_args, root, &[])?;

    let strip_root = |line: &str| -> String {
        line.trim_start_matches(root.as_str())
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
         them:\n- {}",
        truncate(&listing, MAX_DIAGNOSTIC_BYTES)
    )))
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
