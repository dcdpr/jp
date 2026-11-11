use std::collections::{BTreeMap, BTreeSet};

use camino::Utf8Path;
use indoc::indoc;
use jp_process::{ProcessOutput, ProcessRunner, SystemProcessRunner};
use serde::Deserialize;

use super::{MAX_DIAGNOSTIC_BYTES, cargo_scope, comfort_scope};
use crate::util::{ToolResult, error, truncate};

/// Run clippy and comfort over `packages`, or the whole workspace when it is
/// empty.
///
/// With `wiki_check`, `jp wiki check` runs over the workspace's wiki as well,
/// and with `qual_check`, the rustqual gate that CI runs.
pub(crate) async fn cargo_check(
    root: &Utf8Path,
    rustflags: &str,
    profile: Option<&str>,
    packages: &[String],
    checksum_freshness: bool,
    wiki_check: bool,
    qual_check: bool,
) -> ToolResult {
    cargo_check_impl(
        root,
        rustflags,
        profile,
        packages,
        Checks {
            checksum_freshness,
            wiki: wiki_check,
            qual: qual_check,
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

    /// Run the rustqual gate, `qual diff`, as well.
    qual: bool,
}

fn cargo_check_impl<R: ProcessRunner>(
    root: &Utf8Path,
    rustflags: &str,
    profile: Option<&str>,
    packages: &[String],
    checks: Checks,
    runner: &R,
) -> ToolResult {
    let clippy_scope = cargo_scope(packages, "--workspace");
    let profile_arg = profile.map(|name| format!("--profile={name}"));

    let mut env = vec![("RUSTFLAGS", rustflags)];
    if checks.checksum_freshness {
        // Use content checksums instead of file mtimes for cargo's freshness
        // checks, so that sibling checkouts (git worktrees) sharing a target
        // dir cannot serve each other's stale artifacts. Matches CI. Requires
        // nightly cargo. See rust-lang/cargo#14136.
        env.push(("CARGO_UNSTABLE_CHECKSUM_FRESHNESS", "true"));
    }

    let mut args = vec!["clippy", "--color=never"];
    args.extend(clippy_scope.iter().map(String::as_str));
    args.extend([
        "--quiet",
        // Cargo stops scheduling new crates after the first one fails to
        // compile, so an error in one package would hide the diagnostics of
        // every package not yet started.
        "--keep-going",
        "--all-targets",
        // Matches `just lint-ci`. Code behind an optional feature is not
        // compiled without this, so its lints surface only on CI.
        "--all-features",
    ]);
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
    match comfort_check(root, packages, runner)? {
        ComfortCheck::Clean => {}
        ComfortCheck::Drift(note) => notes.push(note),
        ComfortCheck::Failed(stderr) => {
            return error(format!(
                "comfort failed: {}",
                truncate(&stderr, MAX_DIAGNOSTIC_BYTES)
            ));
        }
    }
    if checks.qual {
        match qual_check(root, packages, profile_arg.as_deref(), &env, runner) {
            QualCheck::Clean => {}
            QualCheck::Findings(note) => notes.push(note),
            // A note, where a failing comfort is an error: an error outcome
            // would drop clippy's diagnostics, and a missing `rustqual` or
            // `main` branch says nothing about the code clippy checked.
            QualCheck::Failed(reason) => notes.push(format!(
                "New rustqual findings weren't checked: {}",
                truncate(reason.trim(), MAX_DIAGNOSTIC_BYTES)
            )),
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
    packages: &[String],
    runner: &R,
) -> Result<ComfortCheck, std::io::Error> {
    // The style comes from `.comfort.toml` at `root`, which comfort runs in.
    let mut comfort_args = vec!["--check", "--list-changed", "--language", "rust"];
    comfort_args.extend(comfort_scope(packages));

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

enum QualCheck {
    /// The working tree has no rustqual finding that CI fails on.
    Clean,
    /// CI fails on the working tree; carries the note listing the findings.
    Findings(String),
    /// The gate reached no verdict; carries the reason.
    Failed(String),
}

/// The verdict `qual diff --format json` prints, reduced to the fields a note
/// reports.
#[derive(Debug, Deserialize)]
struct QualVerdict {
    /// Findings the base revision lacks.
    new: Vec<QualFinding>,

    /// Findings of enforced rules, which fail whether or not they are new.
    enforced: Vec<QualFinding>,

    /// The rules `new` and `enforced` name, by rule ID.
    rules: BTreeMap<String, QualRule>,
}

#[derive(Debug, Deserialize)]
struct QualFinding {
    /// Path below the repository root with `/` separators, or empty for a
    /// finding about the workspace as a whole.
    file: String,

    /// 1-based, or `0` for a finding without a line.
    line: usize,

    rule: String,

    /// The enclosing item, or empty for a finding about a whole file.
    symbol: String,

    message: String,

    /// Set when the base revision has findings of the same rule in the same
    /// item, which `qual` can only compare by count.
    shared: Option<QualShared>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct QualShared {
    /// How many more findings the working tree has than the base revision.
    new: usize,

    /// How many findings the working tree has.
    of: usize,
}

#[derive(Debug, Deserialize)]
struct QualRule {
    title: Option<String>,

    /// How to fix a finding of the rule, from `.config/rustqual/guidance/`.
    guidance: Option<String>,
}

impl QualFinding {
    /// Whether the finding lies in one of `dirs`.
    ///
    /// A finding about the workspace as a whole lies in every directory.
    fn is_under(&self, dirs: &[String]) -> bool {
        self.file.is_empty()
            || dirs.iter().any(|dir| {
                dir.is_empty()
                    || self
                        .file
                        .strip_prefix(dir.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            })
    }

    fn location(&self) -> String {
        match (self.file.as_str(), self.line) {
            ("", _) => "<workspace>".to_owned(),
            (file, 0) => file.to_owned(),
            (file, line) => format!("{file}:{line}"),
        }
    }
}

/// What to do about a listed finding, and the markers rustqual accepts.
const QUAL_ADVICE: &str = indoc! {r#"
    Fix each finding. Don't silence one with a marker, a changed threshold, or an edit under `.config/rustqual/` unless the user asked for that in this conversation. If a finding looks wrong, say so. The markers rustqual accepts:

    - `// qual:allow(<dimension>) reason: "<why>"` above an item suppresses that dimension's findings in it.
    - `// qual:api` above a public item whose callers live outside the workspace.
    - `// qual:test_helper` above an item used only from tests."#};

/// Run the rustqual gate that CI runs, `qual diff`, over the working tree.
///
/// The gate analyses the whole workspace, which dead-code and duplicate
/// detection need.
/// `packages` only selects the findings the note lists; the findings in other
/// packages are counted, since CI fails on those too.
fn qual_check<R: ProcessRunner>(
    root: &Utf8Path,
    packages: &[String],
    profile: Option<&str>,
    env: &[(&str, &str)],
    runner: &R,
) -> QualCheck {
    let verdict = match qual_verdict(root, profile, env, runner) {
        Ok(verdict) => verdict,
        Err(reason) => return QualCheck::Failed(reason),
    };
    if verdict.new.is_empty() && verdict.enforced.is_empty() {
        return QualCheck::Clean;
    }

    let dirs = if packages.is_empty() {
        None
    } else {
        match package_dirs(root, packages, runner) {
            Ok(dirs) => Some(dirs),
            Err(reason) => return QualCheck::Failed(reason),
        }
    };

    QualCheck::Findings(qual_note(&verdict, dirs.as_deref()))
}

/// Run `qual diff` against the working tree's merge base with `main`, and read
/// the verdict it prints.
///
/// The build of `qual` gets `profile` and `env`, the same as the clippy run, so
/// it shares that run's artifacts.
fn qual_verdict<R: ProcessRunner>(
    root: &Utf8Path,
    profile: Option<&str>,
    env: &[(&str, &str)],
    runner: &R,
) -> Result<QualVerdict, String> {
    // CI compares against the remote's `main`. A checkout without that remote
    // has only the local branch.
    let remote = runner
        .run(
            "git",
            &["rev-parse", "--verify", "--quiet", "origin/main^{commit}"],
            root,
        )
        .map_err(|error| format!("`git` couldn't be run: {error}"))?;
    let base = if remote.success() {
        "origin/main"
    } else {
        "main"
    };

    let mut args = vec!["run", "--quiet", "--package", "qual"];
    args.extend(profile);
    args.extend(["--", "diff", "--base", base, "--format", "json"]);

    let output = runner
        .run_with_env("cargo", &args, root, env)
        .map_err(|error| format!("`cargo` couldn't be run: {error}"))?;

    // `qual` exits 0 on a passing verdict and 1 on a failing one, and prints
    // the verdict either way. Any other status means it reached none.
    if !matches!(output.status.code(), Some(0 | 1)) {
        return Err(format!(
            "`qual diff --base {base}` failed: {}",
            output.stderr.trim()
        ));
    }

    serde_json::from_str(&output.stdout)
        .map_err(|error| format!("`qual diff` printed a verdict that can't be read: {error}"))
}

/// The part of `cargo metadata`'s output that locates a package.
#[derive(Debug, Deserialize)]
struct Metadata {
    workspace_root: String,
    packages: Vec<MetadataPackage>,
}

#[derive(Debug, Deserialize)]
struct MetadataPackage {
    name: String,
    manifest_path: String,
}

/// The directory of each package in `packages`, below the workspace root and
/// with `/` separators.
fn package_dirs<R: ProcessRunner>(
    root: &Utf8Path,
    packages: &[String],
    runner: &R,
) -> Result<Vec<String>, String> {
    let output = runner
        .run(
            "cargo",
            &["metadata", "--no-deps", "--format-version=1"],
            root,
        )
        .map_err(|error| format!("`cargo` couldn't be run: {error}"))?;
    if !output.success() {
        return Err(format!("`cargo metadata` failed: {}", output.stderr.trim()));
    }

    let metadata: Metadata = serde_json::from_str(&output.stdout)
        .map_err(|error| format!("`cargo metadata` printed output that can't be read: {error}"))?;
    let workspace = Utf8Path::new(&metadata.workspace_root);

    Ok(metadata
        .packages
        .iter()
        .filter(|package| packages.contains(&package.name))
        .filter_map(|package| {
            let dir = Utf8Path::new(&package.manifest_path).parent()?;
            let dir = dir.strip_prefix(workspace).ok()?;
            Some(
                dir.components()
                    .map(|component| component.as_str())
                    .collect::<Vec<_>>()
                    .join("/"),
            )
        })
        .collect())
}

/// The note for a failing `verdict`: the findings under `dirs`, or all of them
/// when `dirs` is `None`, and how many findings lie elsewhere.
fn qual_note(verdict: &QualVerdict, dirs: Option<&[String]>) -> String {
    let listed = |finding: &&QualFinding| dirs.is_none_or(|dirs| finding.is_under(dirs));
    let new: Vec<_> = verdict.new.iter().filter(listed).collect();
    let enforced: Vec<_> = verdict.enforced.iter().filter(listed).collect();
    let elsewhere = verdict.new.len() + verdict.enforced.len() - new.len() - enforced.len();

    let mut listing = String::new();
    push_findings(&mut listing, "New rustqual findings", &new, &verdict.rules);
    push_findings(
        &mut listing,
        "rustqual findings in enforced rules",
        &enforced,
        &verdict.rules,
    );

    if listing.is_empty() {
        return format!(
            "The `qual` job on CI fails on rustqual findings outside the checked packages \
             ({elsewhere}). Run `cargo_check` without `package` to list them."
        );
    }

    let mut note = format!(
        "The `qual` job on CI fails on these rustqual findings:\n\n{}",
        truncate(listing.trim_end(), MAX_DIAGNOSTIC_BYTES)
    );
    if elsewhere > 0 {
        note.push_str(&format!(
            "\n\nFindings outside the checked packages: {elsewhere}. Run `cargo_check` without \
             `package` to list them."
        ));
    }
    note.push_str("\n\n");
    note.push_str(QUAL_ADVICE);
    note
}

/// Append `findings` to `out` under `heading`, grouped by rule, with each
/// rule's guidance after its findings.
///
/// The layout is that of `qual diff --format ai`.
fn push_findings(
    out: &mut String,
    heading: &str,
    findings: &[&QualFinding],
    rules: &BTreeMap<String, QualRule>,
) {
    if findings.is_empty() {
        return;
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&format!("# {heading} ({})\n", findings.len()));

    let mut by_rule = BTreeMap::<&str, Vec<&QualFinding>>::new();
    for finding in findings.iter().copied() {
        by_rule
            .entry(finding.rule.as_str())
            .or_default()
            .push(finding);
    }

    for (id, findings) in by_rule {
        let rule = rules.get(id);
        match rule.and_then(|rule| rule.title.as_deref()) {
            Some(title) => out.push_str(&format!("\n## {id}: {title}\n\n")),
            None => out.push_str(&format!("\n## {id}\n\n")),
        }

        for finding in findings {
            let location = finding.location();
            match finding.symbol.as_str() {
                "" => out.push_str(&format!("- `{location}`: {}\n", finding.message)),
                symbol => out.push_str(&format!(
                    "- `{location}` in `{symbol}`: {}\n",
                    finding.message
                )),
            }
            if let Some(shared) = finding.shared {
                out.push_str(&format!(
                    "  {} findings share this rule and symbol, {} more than at the base.\n",
                    shared.of, shared.new
                ));
            }
        }

        if let Some(guidance) = rule.and_then(|rule| rule.guidance.as_deref()) {
            out.push_str(&format!("\n{guidance}\n"));
        }
    }
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
