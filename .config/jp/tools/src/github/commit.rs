use chrono::{DateTime, Utc};
use jp_github::models::{commits, repos::DiffEntryStatus};

use super::{
    auth_optional,
    changed_files::{ChangedFile, format_changed_files, format_not_found},
    parse_repo,
};
use crate::{Result, github::handle_404, to_xml, util::OneOrMany};

/// Changed files per page for a single commit.
/// Fixed at 100; the GitHub commit endpoint caps the files array at 300 and
/// paginates the remainder.
const FILES_PER_PAGE: u8 = 100;

pub(crate) async fn github_commit(
    repository: Option<String>,
    reference: String,
    files: Option<OneOrMany<String>>,
    page: Option<u64>,
) -> Result<String> {
    auth_optional().await?;

    let (owner, repo) = parse_repo(repository)?;
    let page = page.unwrap_or(1).max(1);
    let files = files.unwrap_or_default();

    let commit = jp_github::instance()
        .repos(&owner, &repo)
        .get_commit(reference.as_str())
        .page(page)
        .per_page(FILES_PER_PAGE)
        .send()
        .await
        .map_err(|e| {
            handle_404(
                e,
                format!("Commit `{reference}` not found in {owner}/{repo}"),
            )
        })?;

    if files.is_empty() {
        enumerate(page, commit)
    } else {
        fetch(page, commit, &files)
    }
}

/// Commit header fields shared by both modes.
struct Header {
    sha: String,
    message: String,
    author: Option<String>,
    date: Option<DateTime<Utc>>,
}

fn header(commit: &commits::Commit) -> Header {
    let git_author = commit.commit.author.as_ref();
    Header {
        sha: commit.sha.clone(),
        message: commit.commit.message.clone(),
        author: commit
            .author
            .as_ref()
            .map(|u| u.login.clone())
            .or_else(|| git_author.and_then(|a| a.name.clone())),
        date: git_author.and_then(|a| a.date),
    }
}

/// List the commit's changed files without patches, plus metadata and stats.
///
/// The commit header stays structured because its message spans lines; the
/// changed files follow it as a bulleted block.
fn enumerate(page: u64, commit: commits::Commit) -> Result<String> {
    #[derive(serde::Serialize)]
    struct Commit {
        sha: String,
        message: String,
        author: Option<String>,
        date: Option<DateTime<Utc>>,
        additions: u64,
        deletions: u64,
        total: u64,
        page: u64,
        per_page: u8,
    }

    let h = header(&commit);
    let stats = commit.stats.unwrap_or(commits::CommitStats {
        additions: 0,
        deletions: 0,
        total: 0,
    });

    let files: Vec<ChangedFile> = commit
        .files
        .unwrap_or_default()
        .into_iter()
        .map(|f| ChangedFile {
            filename: f.filename,
            status: f.status,
            additions: f.additions,
            deletions: f.deletions,
            previous_filename: f.previous_filename,
        })
        .collect();

    let header = to_xml(Commit {
        sha: h.sha,
        message: h.message,
        author: h.author,
        date: h.date,
        additions: stats.additions,
        deletions: stats.deletions,
        total: stats.total,
        page,
        per_page: FILES_PER_PAGE,
    })?;

    Ok(format!("{header}\n\n{}", format_changed_files(&files)))
}

/// Fetch patches for a specific set of files in the commit.
///
/// Files the page does not contain are listed in a `not_found` block below the
/// patches, with one hint covering all of them.
fn fetch(page: u64, commit: commits::Commit, files: &[String]) -> Result<String> {
    // A patch is a diff body rather than a one-line fact, so matched files keep
    // their structured shape instead of collapsing into a list entry.
    #[derive(serde::Serialize)]
    struct MatchedFile {
        filename: String,
        status: DiffEntryStatus,
        additions: u64,
        deletions: u64,
        changes: u64,
        previous_filename: Option<String>,
        patch: Option<String>,
    }

    #[derive(serde::Serialize)]
    struct Response {
        sha: String,
        message: String,
        page: u64,
        per_page: u8,
        file: Vec<MatchedFile>,
    }

    let h = header(&commit);
    let entries = commit.files.unwrap_or_default();

    let mut matched = Vec::new();
    let mut seen = Vec::with_capacity(entries.len());

    for entry in entries {
        seen.push(entry.filename.clone());
        if files.contains(&entry.filename) {
            matched.push(MatchedFile {
                filename: entry.filename,
                status: entry.status,
                additions: entry.additions,
                deletions: entry.deletions,
                changes: entry.changes,
                previous_filename: entry.previous_filename,
                patch: entry.patch,
            });
        }
    }

    let not_found: Vec<String> = files
        .iter()
        .filter(|requested| !seen.iter().any(|seen| seen == *requested))
        .cloned()
        .collect();

    if matched.is_empty() && !not_found.is_empty() {
        return Ok(format_not_found(&not_found));
    }

    let response = to_xml(Response {
        sha: h.sha,
        message: h.message,
        page,
        per_page: FILES_PER_PAGE,
        file: matched,
    })?;

    if not_found.is_empty() {
        return Ok(response);
    }

    Ok(format!("{response}\n\n{}", format_not_found(&not_found)))
}
