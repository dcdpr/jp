use chrono::{DateTime, Utc};
use mcp_attr::Result;
use octocrab::models;
use url::Url;

use super::auth;
use crate::{
    github::{ORG, REPO},
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

    let issue = octocrab::instance().issues(ORG, REPO).get(number).await?;

    Ok(to_xml(Issue {
        number,
        title: issue.title,
        body: issue.body,
        url: issue.html_url,
        labels: issue.labels.into_iter().map(|label| label.name).collect(),
        author: issue.user.login,
        created_at: issue.created_at,
        closed_at: issue.closed_at,
        linked_pull_request: issue.pull_request.map(|pr| pr.html_url),
    }))
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

    let mut issues = vec![];
    let mut page = octocrab::instance()
        .issues(ORG, REPO)
        .list()
        .per_page(100)
        .send()
        .await?;

    loop {
        let next = page.next.clone();
        for issue in page {
            issues.push(Issue {
                number: issue.number,
                title: issue.title,
                url: issue.html_url,
                labels: issue.labels.into_iter().map(|label| label.name).collect(),
                author: issue.user.login,
                created_at: issue.created_at,
                closed_at: issue.closed_at,
                linked_pull_request: issue.pull_request.map(|pr| pr.html_url),
            });
        }

        page = match octocrab::instance()
            .get_page::<models::issues::Issue>(&next)
            .await?
        {
            Some(next_page) => next_page,
            None => break,
        }
    }

    Ok(to_xml(Issues { issue: issues }))
}
