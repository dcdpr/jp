use std::fs;

use camino::Utf8Path;
use serde::Deserialize;

use crate::util::{
    OneOrMany, ToolResult,
    diff::{text_diff, unified_diff},
    error,
    runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
};

/// Lockfile read before and after the update to report what actually changed.
const LOCKFILE: &str = "Cargo.lock";

/// A package to update: either a bare name, or a name pinned to a version.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum PackageSpec {
    Name(String),
    Pinned(PinnedSpec),
}

/// A package named alongside the exact version to move it to.
///
/// Unknown keys are rejected: a misspelled `version` would otherwise be
/// dropped, turning a request for one exact version into "whatever cargo
/// considers newest".
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PinnedSpec {
    pub name: String,

    #[serde(default)]
    pub version: Option<String>,
}

impl PackageSpec {
    fn name(&self) -> &str {
        match self {
            Self::Name(name) => name,
            Self::Pinned(spec) => &spec.name,
        }
    }

    fn version(&self) -> Option<&str> {
        match self {
            Self::Name(_) => None,
            Self::Pinned(spec) => spec.version.as_deref(),
        }
    }

    /// How the package is named in the report: `serde` or `serde@1.0.210`.
    fn label(&self) -> String {
        match self.version() {
            Some(version) => format!("{}@{version}", self.name()),
            None => self.name().to_owned(),
        }
    }
}

pub(crate) async fn cargo_update(root: &Utf8Path, packages: OneOrMany<PackageSpec>) -> ToolResult {
    cargo_update_impl(root, &packages, &DuctProcessRunner)
}

fn cargo_update_impl<R: ProcessRunner>(
    root: &Utf8Path,
    packages: &[PackageSpec],
    runner: &R,
) -> ToolResult {
    if packages.is_empty() {
        return error("At least one package is required.");
    }

    for package in packages {
        if let Err(message) = check_spec("package name", package.name()) {
            return error(message);
        }

        if let Some(version) = package.version()
            && let Err(message) = check_spec("version", version)
        {
            return error(message);
        }
    }

    let lockfile = root.join(LOCKFILE);
    let before = fs::read_to_string(&lockfile).ok();

    // One invocation per package: `--precise` binds to the whole invocation,
    // and cargo rejects every spec in a batch if any one of them is unknown, so
    // batching would let a single bad name discard the other updates.
    let mut reports = vec![];
    let mut failures = vec![];
    for package in packages {
        let mut args = vec!["update", "--color=never", package.name()];
        if let Some(version) = package.version() {
            args.push("--precise");
            args.push(version);
        }

        let ProcessOutput { stderr, status, .. } = runner.run("cargo", &args, root)?;

        let stripped = strip_ansi_escapes::strip_str(stderr);
        let report = stripped.trim();

        if !status.is_success() {
            let indented = report.replace('\n', "\n  ");
            // `*` rather than `-`: the result is rendered with a diff grammar,
            // which would color a leading `-` as a removed line.
            failures.push(format!("* {}: {indented}", package.label()));
            continue;
        }

        if !report.is_empty() {
            reports.push(report.to_owned());
        }
    }

    let total = packages.len();
    let updated = total - failures.len();

    if updated == 0 {
        return error(format!("cargo update failed:\n{}", failures.join("\n")));
    }

    let after = fs::read_to_string(&lockfile).ok();

    let mut sections = vec![];
    // Counts commands, not version bumps: cargo exits 0 for a package that was
    // already at the newest version its requirement allows.
    if total > 1 || !failures.is_empty() {
        sections.push(format!("{updated}/{total} update commands succeeded."));
    }
    sections.extend(reports);
    if !failures.is_empty() {
        sections.push(format!("Failed:\n{}", failures.join("\n")));
    }

    let change = lockfile_change(before.as_deref(), after.as_deref());
    let has_diff = matches!(change, LockfileChange::Changed(_));
    match change {
        LockfileChange::Changed(diff) => sections.push(diff),
        // Succeeding without moving anything is the outcome most easily
        // misread as a bump, so it is stated rather than left to inference.
        // Suppressed when something failed: "nothing to update" would read as
        // a verdict on the whole call.
        LockfileChange::Unchanged if failures.is_empty() => {
            sections.push("Nothing to update, the lockfile is unchanged.".to_owned());
        }
        LockfileChange::Unchanged | LockfileChange::Unknown => {}
    }

    if sections.is_empty() {
        return Ok("cargo update produced no output.".into());
    }

    let content = sections.join("\n\n");

    // The terminal highlights a tool result as a single language, taken from a
    // fence on the first line. Declaring `diff` colors the lockfile hunks and
    // leaves the surrounding prose plain; with no diff there is nothing to
    // color, so the report goes out as plain text.
    Ok(if has_diff {
        format!("```diff\n{content}\n```").into()
    } else {
        content.into()
    })
}

/// What happened to the lockfile across an update.
#[derive(Debug, PartialEq)]
enum LockfileChange {
    /// The lockfile moved; carries the unified diff.
    Changed(String),

    /// The lockfile was read before and after, and is byte-identical.
    Unchanged,

    /// One of the two snapshots is missing, so nothing can be claimed about
    /// what moved.
    Unknown,
}

/// Compare two lockfile snapshots.
///
/// A snapshot missing on either side yields [`LockfileChange::Unknown`] rather
/// than a diff: a lockfile that did not exist yet would render in full, burying
/// the update report under thousands of added lines.
fn lockfile_change(before: Option<&str>, after: Option<&str>) -> LockfileChange {
    let (Some(before), Some(after)) = (before, after) else {
        return LockfileChange::Unknown;
    };

    if before == after {
        return LockfileChange::Unchanged;
    }

    let diff = text_diff(before, after);
    let unified = unified_diff(&diff, LOCKFILE).to_string();

    // Drop the trailing newline so the diff joins with the other report
    // sections like any other block. `strip_suffix` rather than `trim_end`,
    // because a blank context line is a significant single space.
    LockfileChange::Changed(unified.strip_suffix('\n').unwrap_or(&unified).to_owned())
}

/// Reject values cargo would misread as a flag, or that are blank.
///
/// A value starting with `-` would be consumed as an option, which for `cargo
/// update` can silently widen a targeted update into a lockfile-wide one.
fn check_spec(kind: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("Empty {kind}."));
    }

    if value.starts_with('-') {
        return Err(format!(
            "Invalid {kind} '{value}': must not start with '-'."
        ));
    }

    Ok(())
}

#[cfg(test)]
#[path = "update_tests.rs"]
mod tests;
