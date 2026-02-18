use chrono::{DateTime, Utc};
use url::Url;

use super::auth;
use crate::{
    Result,
    github::{ORG, REPO, handle_404},
    to_xml,
};

pub(crate) async fn github_issues(number: Option<u64>) -> Result<String> {
    auth().await?;

    match number {
        Some(number) => get_issue(number).await,
        None => get_issues().await,
    }
}

async fn get_issue(number: u64) -> Result<String> {
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
    }

    let issue = jp_github::instance()
        .issues(ORG, REPO)
        .get(number)
        .await
        .map_err(|e| handle_404(e, format!("Issue #{number} not found in {ORG}/{REPO}")))?;

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
    })
}

async fn get_issues() -> Result<String> {
    #[derive(serde::Serialize)]
    struct Issues {
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
    }

    let page = jp_github::instance()
        .issues(ORG, REPO)
        .list()
        .per_page(100)
        .send()
        .await?;

    let issue = jp_github::instance()
        .all_pages(page)
        .await?
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
        })
        .collect();

    to_xml(Issues { issue })
}
