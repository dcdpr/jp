use std::cmp::Ordering;

use chrono::{DateTime, Utc};
use jp_github::models::checks::{
    Annotation, AnnotationLevel, CheckConclusion, CheckRun, CheckRuns, CheckStatus, CombinedStatus,
    Status, StatusState,
};

use super::{auth_optional, handle_404, parse_repo, read_token};
use crate::{Error, Result, to_xml, util::truncate};

/// Maximum bytes of diagnostics extracted from a job log.
const MAX_DIAGNOSTIC_BYTES: usize = 32_000;

/// Number of trailing log lines included after the extracted diagnostics.
///
/// Cargo and nextest print their failure summary at the end of the log,
/// followed by the runner's exit code.
const TAIL_LINES: usize = 40;

/// GitHub refuses anonymous requests for a job's log, even on a public
/// repository.
const LOG_NEEDS_TOKEN: &str = "Reading a job's log needs a GitHub token: set `JP_GITHUB_TOKEN` or \
                               `GITHUB_TOKEN`. GitHub refuses anonymous requests for logs, even \
                               on a public repository.";

pub(crate) async fn github_pr_checks(
    repository: Option<String>,
    number: u64,
    job: Option<String>,
) -> Result<String> {
    // Check for a token before making any request to avoid spending rate limit
    // on a PR lookup when the log request will fail with 403.
    if job.is_some() && read_token().is_none() {
        return Err(LOG_NEEDS_TOKEN.into());
    }

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
    let statuses = repos.combined_status(&sha).await?;

    if let Some(job) = job {
        let run = find_run(&runs.check_runs, &statuses.statuses, &job)?;
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
            .map_err(|e| log_error(e, &run.name))?;
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

    format_checks(number, &sha, runs, statuses, annotations)
}

fn log_error(error: jp_github::Error, name: &str) -> Error {
    match &error {
        jp_github::Error::GitHub { source, .. } if source.status_code.as_u16() == 403 => format!(
            "GitHub refused the log of `{name}`: {}. The token may lack read access to the \
             repository's Actions.",
            source.message
        )
        .into(),
        _ => handle_404(error, format!("The log of `{name}` is not available")),
    }
}

/// Find the check run matching `job` by ID or by name.
///
/// Numeric IDs take precedence over names.
/// If multiple check runs share a name (for example, when two workflows define
/// a job with the same name), returns an error listing the matching IDs.
fn find_run<'a>(runs: &'a [CheckRun], statuses: &[Status], job: &str) -> Result<&'a CheckRun> {
    if let Ok(id) = job.parse::<u64>()
        && let Some(run) = runs.iter().find(|run| run.id == id)
    {
        return Ok(run);
    }

    let named: Vec<&CheckRun> = runs.iter().filter(|run| run.name == job).collect();
    match named.as_slice() {
        [run] => return Ok(run),
        [] => {}
        many => {
            let ids = many
                .iter()
                .map(|run| run.id.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "`{job}` names {} checks on the pull request's head commit: {ids}. Pass one of \
                 these IDs as `job`.",
                many.len()
            )
            .into());
        }
    }

    if let Some(status) = statuses.iter().find(|status| status.context == job) {
        return Err(format!(
            "`{job}` is a commit status, not a job, so it has no log. See {}",
            status
                .target_url
                .as_deref()
                .filter(|url| !url.is_empty())
                .unwrap_or("its status on GitHub")
        )
        .into());
    }

    let mut names: Vec<&str> = runs.iter().map(|run| run.name.as_str()).collect();
    names.sort_unstable();
    names.dedup();
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

/// Summary category of a check run or commit status, ordered by display
/// priority (failed first, then pending, then passed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    Failed,
    Pending,
    Passed,
}

impl Outcome {
    fn of_status(state: StatusState) -> Self {
        match state {
            StatusState::Success => Self::Passed,
            StatusState::Pending => Self::Pending,
            StatusState::Error | StatusState::Failure | StatusState::Unknown => Self::Failed,
        }
    }

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
    commit_status: Vec<StatusXml>,
}

#[derive(serde::Serialize)]
struct CheckXml {
    id: u64,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    html_url: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    level: Option<AnnotationLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[derive(serde::Serialize)]
struct StatusXml {
    context: String,
    state: StatusState,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_url: Option<String>,
}

/// Format `runs` and commit `statuses` as XML, sorted with failures first, then
/// pending, then passed.
///
/// `annotations` must be parallel to `runs`.
/// The `failed`, `pending`, and `passed` counts include both check runs and
/// commit statuses.
fn format_checks(
    number: u64,
    sha: &str,
    runs: CheckRuns,
    statuses: CombinedStatus,
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
                html_url: run.html_url.map(String::from),
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

    let mut commit_statuses: Vec<(Outcome, StatusXml)> = statuses
        .statuses
        .into_iter()
        .map(|status| {
            (Outcome::of_status(status.state), StatusXml {
                context: status.context,
                state: status.state,
                description: status.description.filter(|text| !text.is_empty()),
                target_url: status.target_url.filter(|url| !url.is_empty()),
            })
        })
        .collect();
    commit_statuses.sort_by(|(a, a_status), (b, b_status)| match a.cmp(b) {
        Ordering::Equal => a_status.context.cmp(&b_status.context),
        other => other,
    });

    let tally = |wanted: Outcome| {
        checks.iter().filter(|(o, _)| *o == wanted).count()
            + commit_statuses.iter().filter(|(o, _)| *o == wanted).count()
    };
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
        commit_status: commit_statuses
            .into_iter()
            .map(|(_, status)| status)
            .collect(),
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

/// Extract compiler diagnostics and the last [`TAIL_LINES`] lines from `log`.
///
/// Strips timestamps and ANSI escape codes from every line.
/// A diagnostic block starts at a compiler `error:` or `warning:` line or a
/// panic header and continues until the next blank line, diagnostic, or cargo
/// progress line (`Compiling`, `Documenting`, etc.).
/// Runner `##[error]` lines are captured as single-line diagnostics.
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
    let mut header = format!(
        "Log of `{}` (job {}), {} lines.",
        run.name,
        run.id,
        lines.len()
    );
    if diagnostics.is_empty() {
        header.push_str(" It has no compiler diagnostics.");
    }
    // The summary includes only extracted diagnostics and the log tail, so
    // failures without standard diagnostic headers (such as a formatter diff)
    // require inspecting the full log.
    if let Some(url) = &run.html_url {
        header.push_str(&format!("\nFull log: {url}"));
    }

    if diagnostics.is_empty() {
        return format!("{header}\n\nLast {TAIL_LINES} lines:\n```text\n{tail}\n```");
    }

    format!(
        "{header}\n\nDiagnostics:\n```text\n{}\n```\n\nLast {TAIL_LINES} \
         lines:\n```text\n{tail}\n```",
        truncate(&diagnostics, MAX_DIAGNOSTIC_BYTES)
    )
}

/// Strip a leading UTF-8 BOM, GitHub timestamp prefix, ANSI escape codes, and
/// trailing whitespace from `line`.
fn clean_line(line: &str) -> String {
    let line = line.trim_start_matches('\u{feff}');
    let line = match line.split_once(' ') {
        Some((stamp, rest)) if is_timestamp(stamp) => rest,
        _ if is_timestamp(line) => "",
        _ => line,
    };

    strip_ansi_escapes::strip_str(line).trim_end().to_owned()
}

/// Return whether `word` is an RFC 3339 UTC timestamp prefix as formatted in
/// GitHub Actions logs (`2026-11-25T10:00:00.1234567Z`).
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

    // Strip an optional lint or error code, such as `error[E0308]:`.
    let rest = match rest.strip_prefix('[') {
        Some(code) => code.split_once(']').map_or("", |(_, rest)| rest),
        None => rest,
    };

    rest.starts_with(": ")
}

/// Return whether `line` is an indented cargo progress line (e.g. `   Compiling
/// foo v0.1.0`).
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
