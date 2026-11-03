use std::collections::BTreeSet;

use camino::Utf8Path;
use jp_process::{ProcessOutput, ProcessRunner, SystemProcessRunner};

use super::MAX_DIAGNOSTIC_BYTES;
use crate::util::{ToolResult, error, truncate};

/// Run clippy and comfort over the workspace or one package, and, with
/// `wiki_check`, `jp wiki check` over the workspace's wiki.
pub(crate) async fn cargo_check(
    root: &Utf8Path,
    rustflags: &str,
    profile: Option<&str>,
    package: Option<String>,
    checksum_freshness: bool,
    wiki_check: bool,
) -> ToolResult {
    cargo_check_impl(
        root,
        rustflags,
        profile,
        package.as_deref(),
        Checks {
            checksum_freshness,
            wiki: wiki_check,
        },
        &SystemProcessRunner,
    )
}

/// What a check run does besides clippy and comfort.
#[derive(Debug, Clone, Copy, Default)]
struct Checks {
    /// Use content checksums for cargo's freshness checks.
    checksum_freshness: bool,

    /// Run `jp wiki check` as well.
    wiki: bool,
}

fn cargo_check_impl<R: ProcessRunner>(
    root: &Utf8Path,
    rustflags: &str,
    profile: Option<&str>,
    package: Option<&str>,
    checks: Checks,
    runner: &R,
) -> ToolResult {
    let clippy_scope = package.map_or("--workspace".to_owned(), |v| format!("--package={v}"));
    let profile_arg = profile.map(|name| format!("--profile={name}"));

    let mut env = vec![("RUSTFLAGS", rustflags)];
    if checks.checksum_freshness {
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

    let mut notes = vec![];
    match comfort_check(root, package, runner)? {
        ComfortCheck::Clean => {}
        ComfortCheck::Drift(note) => notes.push(note),
        ComfortCheck::Failed(stderr) => {
            return error(format!(
                "comfort failed: {}",
                truncate(&stderr, MAX_DIAGNOSTIC_BYTES)
            ));
        }
    }
    if checks.wiki {
        notes.extend(wiki_check(root, runner));
    }

    if notes.is_empty() {
        return Ok(if clippy.is_empty() {
            "Check succeeded. No warnings or errors found."
                .to_owned()
                .into()
        } else {
            format!("```\n{clippy}\n```\n").into()
        });
    }

    // The header is scoped to what clippy alone found. A bare "Check succeeded"
    // would contradict the notes that follow.
    let header = if clippy.is_empty() {
        "`cargo clippy` found no warnings or errors.".to_owned()
    } else {
        format!("```\n{clippy}\n```")
    };

    Ok(format!("{header}\n\n{}", notes.join("\n\n")).into())
}

/// Run `jp wiki check` over the workspace, returning what it found as a note;
/// `None` when the wiki has no problems.
///
/// A wiki page links to source, so a code change can break one; checking the
/// wiki with the code catches that.
/// A wiki that can't be checked at all, with `jp` or `jp-wiki` missing, is a
/// note too, rather than a failed check: the code itself was checked.
fn wiki_check<R: ProcessRunner>(root: &Utf8Path, runner: &R) -> Option<String> {
    let output = match runner.run("jp", &["--no-interactive", "wiki", "check"], root) {
        Ok(output) => output,
        Err(error) => {
            return Some(format!(
                "The wiki wasn't checked: `jp` couldn't be run: {error}"
            ));
        }
    };

    let findings = output
        .stdout
        .lines()
        .any(|line| line.starts_with("error:") || line.starts_with("warning:"));
    match output.status.code() {
        Some(0) if !findings => None,
        Some(0 | 1) => Some(format!(
            "`jp wiki check` found problems:\n```\n{}\n```",
            truncate(output.stdout.trim(), MAX_DIAGNOSTIC_BYTES)
        )),
        _ => Some(format!(
            "The wiki wasn't checked: `jp wiki check` failed: {}",
            truncate(output.stderr.trim(), MAX_DIAGNOSTIC_BYTES)
        )),
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
