use std::cmp::Ordering;

use chrono::{DateTime, Utc};
use jp_github::models::checks::{
    Annotation, AnnotationLevel, CheckConclusion, CheckRun, CheckRuns, CheckStatus,
};

use super::{auth_optional, handle_404, parse_repo};
use crate::{Result, to_xml, util::truncate};

/// Cap for the diagnostics a job's log contributes to the result.
const MAX_DIAGNOSTIC_BYTES: usize = 32_000;

/// Lines of a job's log returned after its diagnostics.
///
/// The end of a failed job's log is where cargo and nextest summarize the
/// failure and the runner reports the exit code.
const TAIL_LINES: usize = 40;

pub(crate) async fn github_pr_checks(
    repository: Option<String>,
    number: u64,
    job: Option<String>,
) -> Result<String> {
    auth_optional().await?;

    let (owner, repo) = parse_repo(repository)?;
    let client = jp_github::instance();

    let pull = client
        .pulls(&owner, &repo)
        .get(number)
        .await
        .map_err(|e| handle_404(e, format!("Pull #{number} not found in {owner}/{repo}")))?;
    let sha = pull
        .head
        .map(|head| head.sha)
        .ok_or_else(|| format!("Pull #{number} in {owner}/{repo} has no head commit"))?;

    let repos = client.repos(&owner, &repo);
    let runs = repos.list_check_runs(&sha).await?;

    if let Some(job) = job {
        let run = find_run(&runs.check_runs, &job)?;
        if !run.is_actions_job() {
            return Err(format!(
                "`{}` is not a GitHub Actions job, so it has no log. See {}",
                run.name,
                run.html_url
                    .as_ref()
                    .map_or("its check run on GitHub", |url| url.as_str())
            )
            .into());
        }

        let log = repos
            .job_logs(run.id)
            .await
            .map_err(|e| handle_404(e, format!("The log of `{}` is not available", run.name)))?;
        return Ok(summarize_log(run, &log));
    }

    let mut annotations = Vec::with_capacity(runs.check_runs.len());
    for run in &runs.check_runs {
        if run.output.annotations_count == 0 {
            annotations.push(vec![]);
            continue;
        }
        annotations.push(repos.list_check_run_annotations(run.id).await?);
    }

    format_checks(number, &sha, runs, annotations)
}

/// The check run `job` names, by ID or by name.
fn find_run<'a>(runs: &'a [CheckRun], job: &str) -> Result<&'a CheckRun> {
    let by_id = job.parse::<u64>().ok();
    if let Some(run) = runs
        .iter()
        .find(|run| Some(run.id) == by_id || run.name == job)
    {
        return Ok(run);
    }

    let mut names: Vec<&str> = runs.iter().map(|run| run.name.as_str()).collect();
    names.sort_unstable();
    Err(format!(
        "No check named `{job}` on the pull request's head commit. Its checks: {}",
        names
            .iter()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(", ")
    )
    .into())
}

/// Where a check run stands, in the order the listing shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    Failed,
    Pending,
    Passed,
}

impl Outcome {
    fn of(run: &CheckRun) -> Self {
        if run.status != CheckStatus::Completed {
            return Self::Pending;
        }
        match run.conclusion {
            Some(
                CheckConclusion::Success | CheckConclusion::Neutral | CheckConclusion::Skipped,
            ) => Self::Passed,
            _ => Self::Failed,
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename = "Checks")]
struct ChecksXml {
    number: u64,
    sha: String,
    failed: usize,
    pending: usize,
    passed: usize,
    /// Set when the commit has more check runs than the one page fetched.
    #[serde(skip_serializing_if = "Option::is_none")]
    total_count: Option<u64>,
    check: Vec<CheckXml>,
}

#[derive(serde::Serialize)]
struct CheckXml {
    id: u64,
    name: String,
    status: CheckStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    conclusion: Option<CheckConclusion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    started_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_at: Option<DateTime<Utc>>,
    annotation: Vec<AnnotationXml>,
}

#[derive(serde::Serialize)]
struct AnnotationXml {
    location: String,
    level: AnnotationLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    message: String,
}

/// List `runs`, failed first, then pending, then passed, each with its
/// `annotations` (in the same order as `runs`).
fn format_checks(
    number: u64,
    sha: &str,
    runs: CheckRuns,
    annotations: Vec<Vec<Annotation>>,
) -> Result<String> {
    let count = runs.check_runs.len();
    let mut checks: Vec<(Outcome, CheckXml)> = runs
        .check_runs
        .into_iter()
        .zip(annotations)
        .map(|(run, annotations)| {
            let outcome = Outcome::of(&run);
            let annotation = annotations
                .into_iter()
                .map(|annotation| AnnotationXml {
                    location: location(&annotation),
                    level: annotation.annotation_level,
                    title: annotation.title.filter(|title| !title.is_empty()),
                    message: annotation.message,
                })
                .collect();

            (outcome, CheckXml {
                id: run.id,
                name: run.name,
                status: run.status,
                conclusion: run.conclusion,
                started_at: run.started_at,
                completed_at: run.completed_at,
                annotation,
            })
        })
        .collect();

    checks.sort_by(|(a, a_check), (b, b_check)| match a.cmp(b) {
        Ordering::Equal => a_check.name.cmp(&b_check.name),
        other => other,
    });

    let tally = |wanted: Outcome| checks.iter().filter(|(o, _)| *o == wanted).count();
    let (failed, pending, passed) = (
        tally(Outcome::Failed),
        tally(Outcome::Pending),
        tally(Outcome::Passed),
    );

    to_xml(ChecksXml {
        number,
        sha: sha.to_owned(),
        failed,
        pending,
        passed,
        total_count: u64::try_from(count)
            .is_ok_and(|count| runs.total_count > count)
            .then_some(runs.total_count),
        check: checks.into_iter().map(|(_, check)| check).collect(),
    })
}

fn location(annotation: &Annotation) -> String {
    if annotation.start_line == annotation.end_line {
        return format!("{}:{}", annotation.path, annotation.start_line);
    }

    format!(
        "{}:{}-{}",
        annotation.path, annotation.start_line, annotation.end_line
    )
}

/// The diagnostics in a job's `log`, followed by its last [`TAIL_LINES`] lines.
///
/// Timestamps and ANSI escapes are removed from every line.
/// A diagnostic is a compiler `error:` or `warning:` line, or a panic, with the
/// lines that belong to it: up to the next blank line, the next diagnostic, or
/// the next cargo progress line (`Compiling`, `Documenting`, ...).
/// A runner `##[error]` line is a diagnostic of its own.
fn summarize_log(run: &CheckRun, log: &str) -> String {
    let lines: Vec<String> = log.lines().map(clean_line).collect();

    let mut blocks: Vec<Vec<&str>> = vec![];
    let mut in_block = false;
    for line in lines.iter().map(String::as_str) {
        if line.starts_with("##[error]") {
            blocks.push(vec![line]);
            in_block = false;
        } else if starts_diagnostic(line) {
            blocks.push(vec![line]);
            in_block = true;
        } else if in_block
            && !line.trim().is_empty()
            && !is_cargo_progress(line)
            && let Some(block) = blocks.last_mut()
        {
            block.push(line);
        } else {
            in_block = false;
        }
    }
    let diagnostics = blocks
        .iter()
        .map(|block| block.join("\n"))
        .collect::<Vec<_>>()
        .join("\n\n");

    let tail = lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n");
    let header = format!(
        "Log of `{}` (job {}), {} lines.",
        run.name,
        run.id,
        lines.len()
    );

    if diagnostics.is_empty() {
        return format!(
            "{header} It has no compiler diagnostics.\n\nLast {TAIL_LINES} \
             lines:\n```text\n{tail}\n```"
        );
    }

    format!(
        "{header}\n\nDiagnostics:\n```text\n{}\n```\n\nLast {TAIL_LINES} \
         lines:\n```text\n{tail}\n```",
        truncate(&diagnostics, MAX_DIAGNOSTIC_BYTES)
    )
}

/// `line` without the timestamp GitHub prefixes it with, the byte-order mark
/// the log starts with, or ANSI escapes.
fn clean_line(line: &str) -> String {
    let line = line.trim_start_matches('\u{feff}');
    let line = match line.split_once(' ') {
        Some((stamp, rest)) if is_timestamp(stamp) => rest,
        _ if is_timestamp(line) => "",
        _ => line,
    };

    strip_ansi_escapes::strip_str(line).trim_end().to_owned()
}

/// Whether `word` is an RFC 3339 UTC timestamp, as GitHub writes them:
/// `2026-11-25T10:00:00.1234567Z`.
fn is_timestamp(word: &str) -> bool {
    word.ends_with('Z')
        && word.get(4..5) == Some("-")
        && word.get(10..11) == Some("T")
        && word.starts_with(|c: char| c.is_ascii_digit())
}

fn starts_diagnostic(line: &str) -> bool {
    // `thread 'name' panicked at`, or `thread 'name' (id) panicked at`.
    if line.trim_start().starts_with("thread '") && line.contains(" panicked at ") {
        return true;
    }

    let rest = line
        .strip_prefix("error")
        .or_else(|| line.strip_prefix("warning"));
    let Some(rest) = rest else {
        return false;
    };

    // The lint or error code, as in `error[E0308]:`.
    let rest = match rest.strip_prefix('[') {
        Some(code) => code.split_once(']').map_or("", |(_, rest)| rest),
        None => rest,
    };

    rest.starts_with(": ")
}

/// Whether `line` is one of cargo's progress lines, which it right-aligns: `
/// Compiling foo v0.1.0`.
fn is_cargo_progress(line: &str) -> bool {
    const VERBS: &[&str] = &[
        "Blocking",
        "Checking",
        "Compiling",
        "Documenting",
        "Downloaded",
        "Downloading",
        "Finished",
        "Fresh",
        "Generated",
        "Running",
    ];

    if !line.starts_with(' ') {
        return false;
    }

    let verb = line.trim_start().split(' ').next().unwrap_or_default();
    VERBS.contains(&verb)
}

#[cfg(test)]
#[path = "pr_checks_tests.rs"]
mod tests;
