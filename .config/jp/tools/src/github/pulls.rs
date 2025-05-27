use chrono::{DateTime, Utc};
use mcp_attr::Result;
use octocrab::{models, params};
use schemars::JsonSchema;
use url::Url;

use super::auth;
use crate::{
    github::{ORG, REPO},
    to_xml,
};

/// The status of a issue or pull request.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Open,
    Closed,
}

pub(crate) async fn github_pulls(number: Option<u64>, state: Option<State>) -> Result<String> {
    auth().await?;

    match number {
        Some(number) => get(number).await,
        None => list(state).await,
    }
}

async fn get(number: u64) -> Result<String> {
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
        diff: String,
    }

    let pull = octocrab::instance().pulls(ORG, REPO).get(number).await?;

    Ok(to_xml(Pull {
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
        diff: octocrab::instance()
            .pulls(ORG, REPO)
            .get_diff(number)
            .await?,
    }))
}

async fn list(state: Option<State>) -> Result<String> {
    #[derive(serde::Serialize)]
    struct Pulls {
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

    let mut pulls = vec![];
    let state = match state {
        Some(State::Open) => params::State::Open,
        Some(State::Closed) => params::State::Closed,
        None => params::State::All,
    };

    let mut page = octocrab::instance()
        .pulls(ORG, REPO)
        .list()
        .state(state)
        .per_page(100)
        .send()
        .await?;

    loop {
        let next = page.next.clone();
        for pull in page {
            pulls.push(Pull {
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
            });
        }

        page = match octocrab::instance()
            .get_page::<models::pulls::PullRequest>(&next)
            .await?
        {
            Some(next_page) => next_page,
            None => break,
        }
    }

    Ok(to_xml(Pulls { pull: pulls }))
}
