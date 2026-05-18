use jp_github::models::repos::DiffEntryStatus;

use super::{auth_optional, parse_repo};
use crate::{Result, github::handle_404, to_xml, to_xml_with_root, util::OneOrMany};

/// Files per page when enumerating changed files. Fixed at 100 (the
/// GitHub API max for `/pulls/{N}/files`).
const FILES_PER_PAGE: u8 = 100;

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
/// The `patch` field is intentionally omitted here — for a typical PR
/// (dozens of files) the patches together easily blow the LLM context
/// window. The caller picks which files they actually need and re-calls
/// with `files: [...]` to get those patches specifically.
async fn enumerate(owner: &str, repo: &str, number: u64, page: u64) -> Result<String> {
    #[derive(serde::Serialize)]
    struct ChangedFile {
        filename: String,
        status: DiffEntryStatus,
        additions: u64,
        deletions: u64,
        changes: u64,
        previous_filename: Option<String>,
    }

    #[derive(serde::Serialize)]
    struct Files {
        number: u64,
        page: u64,
        per_page: u8,
        changed_files_count: u64,
        file: Vec<ChangedFile>,
    }

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

    let file = entries
        .into_iter()
        .map(|entry| ChangedFile {
            filename: entry.filename,
            status: entry.status,
            additions: entry.additions,
            deletions: entry.deletions,
            changes: entry.changes,
            previous_filename: entry.previous_filename,
        })
        .collect();

    to_xml(Files {
        number,
        page,
        per_page: FILES_PER_PAGE,
        changed_files_count: pull.changed_files,
        file,
    })
}

/// Fetch patches for a specific set of files.
///
/// Searches a single page of the changed-files list (per `page`) and
/// returns matching files with their `patch` field included. Files not
/// found on the requested page get an explicit `not_found` entry — the
/// LLM can either bump `page` or call `enumerate` mode to find which
/// page each file lives on.
async fn fetch(
    owner: &str,
    repo: &str,
    number: u64,
    files: Vec<String>,
    page: u64,
) -> Result<String> {
    #[derive(serde::Serialize)]
    struct ChangedFile {
        filename: String,
        status: DiffEntryStatus,
        additions: u64,
        deletions: u64,
        changes: u64,
        previous_filename: Option<String>,
        patch: Option<String>,
    }

    #[derive(serde::Serialize)]
    struct NotFound {
        filename: String,
        // Hint string to nudge the LLM toward the right next call.
        hint: &'static str,
    }

    #[derive(serde::Serialize)]
    struct Response {
        number: u64,
        page: u64,
        per_page: u8,
        file: Vec<ChangedFile>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        not_found: Vec<NotFound>,
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
            matched.push(ChangedFile {
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

    let not_found: Vec<NotFound> = files
        .iter()
        .filter(|requested| !seen_filenames.iter().any(|seen| seen == *requested))
        .map(|filename| NotFound {
            filename: filename.clone(),
            hint: "not present on this page; bump `page` or call without `files` to enumerate \
                   changed files and locate the right page",
        })
        .collect();

    if matched.is_empty() && !not_found.is_empty() {
        // Render only the not-found block so the LLM gets a clear empty
        // result rather than an XML with one filler element.
        return to_xml_with_root(&not_found, "not_found");
    }

    to_xml(Response {
        number,
        page,
        per_page: FILES_PER_PAGE,
        file: matched,
        not_found,
    })
}
