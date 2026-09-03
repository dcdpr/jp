use jp_github::models::repos::DiffEntryStatus;

use super::{
    auth_optional,
    changed_files::{ChangedFile, format_changed_files, format_not_found},
    parse_repo,
};
use crate::{Result, github::handle_404, to_xml, util::OneOrMany};

/// Files per page when enumerating changed files.
/// Fixed at 100 (the GitHub API max for `/pulls/{N}/files`).
const FILES_PER_PAGE: u8 = 100;

/// Render one page of changed files: a header naming the page, then the files.
///
/// The header carries the total count because a page holding exactly
/// `FILES_PER_PAGE` entries is otherwise indistinguishable from the last one.
fn format_enumeration(number: u64, page: u64, total: u64, files: &[ChangedFile]) -> String {
    format!(
        "Pull #{number}, page {page} of {total} changed files ({FILES_PER_PAGE} per page).\n\n{}",
        format_changed_files(files)
    )
}

pub(crate) async fn github_pr_diff(
    repository: Option<String>,
    number: u64,
    files: Option<OneOrMany<String>>,
    page: Option<u64>,
) -> Result<String> {
    auth_optional().await?;

    let (owner, repo) = parse_repo(repository)?;
    let page = page.unwrap_or(1).max(1);
    let files = files.unwrap_or_default();

    if files.is_empty() {
        enumerate(&owner, &repo, number, page).await
    } else {
        fetch(&owner, &repo, number, files.into_vec(), page).await
    }
}

/// List a page of changed files without their patches.
///
/// The `patch` field is intentionally omitted here — for a typical PR (dozens
/// of files) the patches together easily blow the LLM context window.
/// The caller picks which files they actually need and re-calls with `files:
/// [...]` to get those patches specifically.
async fn enumerate(owner: &str, repo: &str, number: u64, page: u64) -> Result<String> {
    let client = jp_github::instance();

    // We fetch PR metadata first solely to get the authoritative
    // `changed_files` count; the LLM otherwise has no way to know whether
    // page 1 of 100 entries exhausted the PR or not.
    let pull = client
        .pulls(owner, repo)
        .get(number)
        .await
        .map_err(|e| handle_404(e, format!("Pull #{number} not found in {owner}/{repo}")))?;

    let entries = client
        .pulls(owner, repo)
        .list_files(number)
        .page(page)
        .per_page(FILES_PER_PAGE)
        .send()
        .await
        .map_err(|e| handle_404(e, format!("Pull #{number} not found in {owner}/{repo}")))?;

    let files: Vec<ChangedFile> = entries
        .into_iter()
        .map(|entry| ChangedFile {
            filename: entry.filename,
            status: entry.status,
            additions: entry.additions,
            deletions: entry.deletions,
            previous_filename: entry.previous_filename,
        })
        .collect();

    Ok(format_enumeration(number, page, pull.changed_files, &files))
}

/// Fetch patches for a specific set of files.
///
/// Searches a single page of the changed-files list (per `page`) and returns
/// matching files with their `patch` field included.
/// Files the page does not contain are listed in a `not_found` block below the
/// patches, with one hint covering all of them.
async fn fetch(
    owner: &str,
    repo: &str,
    number: u64,
    files: Vec<String>,
    page: u64,
) -> Result<String> {
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
        number: u64,
        page: u64,
        per_page: u8,
        file: Vec<MatchedFile>,
    }

    let entries = jp_github::instance()
        .pulls(owner, repo)
        .list_files(number)
        .page(page)
        .per_page(FILES_PER_PAGE)
        .send()
        .await
        .map_err(|e| handle_404(e, format!("Pull #{number} not found in {owner}/{repo}")))?;

    let mut matched = Vec::new();
    let mut seen_filenames = Vec::with_capacity(entries.len());

    for entry in entries {
        seen_filenames.push(entry.filename.clone());
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
        .filter(|requested| !seen_filenames.iter().any(|seen| seen == *requested))
        .cloned()
        .collect();

    if matched.is_empty() && !not_found.is_empty() {
        // Render only the not-found block so the LLM gets a clear empty
        // result rather than an XML with one filler element.
        return Ok(format_not_found(&not_found));
    }

    let response = to_xml(Response {
        number,
        page,
        per_page: FILES_PER_PAGE,
        file: matched,
    })?;

    if not_found.is_empty() {
        return Ok(response);
    }

    Ok(format!("{response}\n\n{}", format_not_found(&not_found)))
}

#[cfg(test)]
#[path = "pr_diff_tests.rs"]
mod tests;
