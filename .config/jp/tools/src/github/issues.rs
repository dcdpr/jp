use chrono::{DateTime, Utc};
use url::Url;

use super::{auth_optional, parse_repo};
use crate::{Result, github::handle_404, to_xml};

/// Comments-per-page when fetching a specific issue. Fixed at 10 to keep
/// responses bounded; long threads are walked with the `page` parameter.
const COMMENTS_PER_PAGE: u8 = 10;

pub(crate) async fn github_issues(
    repository: Option<String>,
    number: Option<u64>,
    page: Option<u64>,
) -> Result<String> {
    auth_optional().await?;

    let (owner, repo) = parse_repo(repository)?;
    let page = page.unwrap_or(1).max(1);

    match number {
        Some(number) => get_issue(&owner, &repo, number, page).await,
        None => get_issues(&owner, &repo, page).await,
    }
}

async fn get_issue(owner: &str, repo: &str, number: u64, page: u64) -> Result<String> {
    #[derive(serde::Serialize)]
    struct Comment {
        author: String,
        created_at: DateTime<Utc>,
        body: Option<String>,
    }

    #[derive(serde::Serialize)]
    struct Issue {
        number: u64,
        title: String,
        body: Option<String>,
        url: Url,
        labels: Vec<String>,
        author: String,
        created_at: DateTime<Utc>,
        closed_at: Option<DateTime<Utc>>,
        linked_pull_request: Option<Url>,
        comments_count: u64,
        comments_page: u64,
        comments_per_page: u8,
        comments: Vec<Comment>,
    }

    let client = jp_github::instance();

    let issue = client
        .issues(owner, repo)
        .get(number)
        .await
        .map_err(|e| handle_404(e, format!("Issue #{number} not found in {owner}/{repo}")))?;

    let comments = client
        .issues(owner, repo)
        .list_comments(number)
        .page(page)
        .per_page(COMMENTS_PER_PAGE)
        .send()
        .await
        .map_err(|e| handle_404(e, format!("Issue #{number} not found in {owner}/{repo}")))?
        .into_iter()
        .map(|c| Comment {
            author: c.user.login,
            created_at: c.created_at,
            body: c.body,
        })
        .collect();

    to_xml(Issue {
        number,
        title: issue.title,
        body: issue.body,
        url: issue.html_url,
        labels: issue.labels.into_iter().map(|label| label.name).collect(),
        author: issue.user.login,
        created_at: issue.created_at,
        closed_at: issue.closed_at,
        linked_pull_request: issue.pull_request.map(|pr| pr.html_url),
        comments_count: issue.comments,
        comments_page: page,
        comments_per_page: COMMENTS_PER_PAGE,
        comments,
    })
}

/// Items per page when listing issues. Fixed at 100 (the GitHub API
/// max for this endpoint) so a single response covers as much ground
/// as possible while staying bounded.
const LIST_PER_PAGE: u8 = 100;

async fn get_issues(owner: &str, repo: &str, page: u64) -> Result<String> {
    #[derive(serde::Serialize)]
    struct Issues {
        page: u64,
        per_page: u8,
        issue: Vec<Issue>,
    }

    #[derive(serde::Serialize)]
    struct Issue {
        number: u64,
        title: String,
        url: Url,
        labels: Vec<String>,
        author: String,
        created_at: DateTime<Utc>,
        closed_at: Option<DateTime<Utc>>,
        linked_pull_request: Option<Url>,
        comments_count: u64,
    }

    let issues = jp_github::instance()
        .issues(owner, repo)
        .list()
        .page(page)
        .per_page(LIST_PER_PAGE)
        .send()
        .await?;

    let issue = issues
        .into_iter()
        .map(|issue| Issue {
            number: issue.number,
            title: issue.title,
            url: issue.html_url,
            labels: issue.labels.into_iter().map(|label| label.name).collect(),
            author: issue.user.login,
            created_at: issue.created_at,
            closed_at: issue.closed_at,
            linked_pull_request: issue.pull_request.map(|pr| pr.html_url),
            comments_count: issue.comments,
        })
        .collect();

    to_xml(Issues {
        page,
        per_page: LIST_PER_PAGE,
        issue,
    })
}
