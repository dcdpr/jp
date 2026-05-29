use chrono::{DateTime, Utc};
use jp_github::params;
use url::Url;

use super::{State, auth_optional, parse_repo};
use crate::{Result, github::handle_404, to_xml};

/// Comments-per-page when fetching a specific pull request.
/// Matches the issues tool — long discussions are walked with the `page`
/// parameter.
const COMMENTS_PER_PAGE: u8 = 10;

pub(crate) async fn github_pulls(
    repository: Option<String>,
    number: Option<u64>,
    state: Option<State>,
    page: Option<u64>,
) -> Result<String> {
    auth_optional().await?;

    let (owner, repo) = parse_repo(repository)?;
    let page = page.unwrap_or(1).max(1);

    match number {
        Some(number) => get(&owner, &repo, number, page).await,
        None => list(&owner, &repo, state, page).await,
    }
}

async fn get(owner: &str, repo: &str, number: u64, page: u64) -> Result<String> {
    #[derive(serde::Serialize)]
    struct Comment {
        author: String,
        created_at: DateTime<Utc>,
        body: Option<String>,
    }

    #[derive(serde::Serialize)]
    struct Pull {
        number: u64,
        title: Option<String>,
        body: Option<String>,
        url: Option<Url>,
        labels: Vec<String>,
        author: Option<String>,
        created_at: Option<DateTime<Utc>>,
        closed_at: Option<DateTime<Utc>>,
        merged_at: Option<DateTime<Utc>>,
        merge_commit_sha: Option<String>,
        changed_files_count: u64,
        comments_count: u64,
        comments_page: u64,
        comments_per_page: u8,
        comments: Vec<Comment>,
    }

    let client = jp_github::instance();

    let pull = client
        .pulls(owner, repo)
        .get(number)
        .await
        .map_err(|e| handle_404(e, format!("Pull #{number} not found in {owner}/{repo}")))?;

    // PR conversation comments share the issues endpoint — `/issues/{N}/comments`
    // returns the same thread shown in the "Conversation" tab. Inline review
    // comments live on a different endpoint and aren't part of this scope.
    let comments = client
        .issues(owner, repo)
        .list_comments(number)
        .page(page)
        .per_page(COMMENTS_PER_PAGE)
        .send()
        .await
        .map_err(|e| handle_404(e, format!("Pull #{number} not found in {owner}/{repo}")))?
        .into_iter()
        .map(|c| Comment {
            author: c.user.login,
            created_at: c.created_at,
            body: c.body,
        })
        .collect();

    to_xml(Pull {
        number,
        title: pull.title,
        body: pull.body,
        url: pull.html_url,
        labels: pull
            .labels
            .into_iter()
            .flatten()
            .map(|label| label.name)
            .collect(),
        author: pull.user.map(|user| user.login),
        created_at: pull.created_at,
        closed_at: pull.closed_at,
        merged_at: pull.merged_at,
        merge_commit_sha: pull.merge_commit_sha,
        changed_files_count: pull.changed_files,
        comments_count: pull.comments,
        comments_page: page,
        comments_per_page: COMMENTS_PER_PAGE,
        comments,
    })
}

/// Items per page when listing pull requests.
/// Fixed at 100 (the GitHub API max for this endpoint).
const LIST_PER_PAGE: u8 = 100;

async fn list(owner: &str, repo: &str, state: Option<State>, page: u64) -> Result<String> {
    #[derive(serde::Serialize)]
    struct Pulls {
        page: u64,
        per_page: u8,
        pull: Vec<Pull>,
    }

    #[derive(serde::Serialize)]
    struct Pull {
        number: u64,
        title: Option<String>,
        url: Option<Url>,
        labels: Vec<String>,
        author: Option<String>,
        created_at: Option<DateTime<Utc>>,
        closed_at: Option<DateTime<Utc>>,
        merged_at: Option<DateTime<Utc>>,
        merge_commit_sha: Option<String>,
    }

    let state = match state {
        Some(State::Open) => params::State::Open,
        Some(State::Closed) => params::State::Closed,
        None => params::State::All,
    };

    let pulls = jp_github::instance()
        .pulls(owner, repo)
        .list()
        .state(state)
        .page(page)
        .per_page(LIST_PER_PAGE)
        .send()
        .await?;

    let pull = pulls
        .into_iter()
        .map(|pull| Pull {
            number: pull.number,
            title: pull.title,
            url: pull.html_url,
            labels: pull
                .labels
                .into_iter()
                .flatten()
                .map(|label| label.name)
                .collect(),
            author: pull.user.map(|user| user.login),
            created_at: pull.created_at,
            closed_at: pull.closed_at,
            merged_at: pull.merged_at,
            merge_commit_sha: pull.merge_commit_sha,
        })
        .collect();

    to_xml(Pulls {
        page,
        per_page: LIST_PER_PAGE,
        pull,
    })
}
