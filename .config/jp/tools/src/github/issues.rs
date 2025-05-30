use time::OffsetDateTime;
use url::Url;

use super::auth;
use crate::{
    github::{handle_404, ORG, REPO},
    to_xml, Result,
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
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        #[serde(with = "time::serde::rfc3339::option")]
        closed_at: Option<OffsetDateTime>,
        linked_pull_request: Option<Url>,
    }

    let issue = octocrab::instance()
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
        created_at: OffsetDateTime::from_unix_timestamp(issue.created_at.timestamp())?,
        closed_at: issue
            .closed_at
            .map(|t| OffsetDateTime::from_unix_timestamp(t.timestamp()))
            .transpose()?,
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
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
        #[serde(with = "time::serde::rfc3339::option")]
        closed_at: Option<OffsetDateTime>,
        linked_pull_request: Option<Url>,
    }

    let page = octocrab::instance()
        .issues(ORG, REPO)
        .list()
        .per_page(100)
        .send()
        .await?;

    let issue = octocrab::instance()
        .all_pages(page)
        .await?
        .into_iter()
        .map(|issue| {
            Ok(Issue {
                number: issue.number,
                title: issue.title,
                url: issue.html_url,
                labels: issue.labels.into_iter().map(|label| label.name).collect(),
                author: issue.user.login,
                created_at: OffsetDateTime::from_unix_timestamp(issue.created_at.timestamp())?,
                closed_at: issue
                    .closed_at
                    .map(|t| OffsetDateTime::from_unix_timestamp(t.timestamp()))
                    .transpose()?,
                linked_pull_request: issue.pull_request.map(|pr| pr.html_url),
            })
        })
        .collect::<Result<_>>()?;

    to_xml(Issues { issue })
}
