use chrono::{DateTime, Utc};

use super::{auth_optional, parse_repo};
use crate::{Result, github::handle_404, to_xml};

/// Commits per page when listing a pull request's commits.
/// Fixed at 100 (the GitHub API max for this endpoint).
const COMMITS_PER_PAGE: u8 = 100;

#[derive(serde::Serialize)]
struct Commit {
    sha: String,
    subject: String,
    author: Option<String>,
    date: Option<DateTime<Utc>>,
}

#[derive(serde::Serialize)]
struct Commits {
    number: u64,
    page: u64,
    per_page: u8,
    commit: Vec<Commit>,
}

pub(crate) async fn github_pr_commits(
    repository: Option<String>,
    number: u64,
    page: Option<u64>,
) -> Result<String> {
    auth_optional().await?;

    let (owner, repo) = parse_repo(repository)?;
    let page = page.unwrap_or(1).max(1);

    let commits = jp_github::instance()
        .pulls(&owner, &repo)
        .list_commits(number)
        .page(page)
        .per_page(COMMITS_PER_PAGE)
        .send()
        .await
        .map_err(|e| handle_404(e, format!("Pull #{number} not found in {owner}/{repo}")))?;

    let commit = commits
        .into_iter()
        .map(|c| {
            let git_author = c.commit.author;
            let date = git_author.as_ref().and_then(|a| a.date);
            // Prefer the linked GitHub login; fall back to the git author name
            // for commits whose email maps to no GitHub account.
            let author = c
                .author
                .map(|u| u.login)
                .or_else(|| git_author.and_then(|a| a.name));

            Commit {
                sha: c.sha,
                subject: c
                    .commit
                    .message
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
                author,
                date,
            }
        })
        .collect();

    to_xml(Commits {
        number,
        page,
        per_page: COMMITS_PER_PAGE,
        commit,
    })
}
