use octocrab::params;
use time::OffsetDateTime;
use url::Url;

use super::auth;
use crate::{
    github::{handle_404, ORG, REPO},
    to_xml, Result,
};

/// The status of a issue or pull request.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
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
        #[serde(with = "time::serde::rfc3339::option")]
        created_at: Option<OffsetDateTime>,
        #[serde(with = "time::serde::rfc3339::option")]
        closed_at: Option<OffsetDateTime>,
        #[serde(with = "time::serde::rfc3339::option")]
        merged_at: Option<OffsetDateTime>,
        merge_commit_sha: Option<String>,
        diff: String,
    }

    let pull = octocrab::instance()
        .pulls(ORG, REPO)
        .get(number)
        .await
        .map_err(|e| handle_404(e, format!("Pull #{number} not found in {ORG}/{REPO}")))?;

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
        created_at: pull
            .created_at
            .map(|v| OffsetDateTime::from_unix_timestamp(v.timestamp()))
            .transpose()?,
        closed_at: pull
            .closed_at
            .map(|v| OffsetDateTime::from_unix_timestamp(v.timestamp()))
            .transpose()?,
        merged_at: pull
            .merged_at
            .map(|v| OffsetDateTime::from_unix_timestamp(v.timestamp()))
            .transpose()?,
        merge_commit_sha: pull.merge_commit_sha,
        diff: octocrab::instance()
            .pulls(ORG, REPO)
            .get_diff(number)
            .await?,
    })
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
        #[serde(with = "time::serde::rfc3339::option")]
        created_at: Option<OffsetDateTime>,
        #[serde(with = "time::serde::rfc3339::option")]
        closed_at: Option<OffsetDateTime>,
        #[serde(with = "time::serde::rfc3339::option")]
        merged_at: Option<OffsetDateTime>,
        merge_commit_sha: Option<String>,
    }

    let state = match state {
        Some(State::Open) => params::State::Open,
        Some(State::Closed) => params::State::Closed,
        None => params::State::All,
    };

    let page = octocrab::instance()
        .pulls(ORG, REPO)
        .list()
        .state(state)
        .per_page(100)
        .send()
        .await?;

    let pull = octocrab::instance()
        .all_pages(page)
        .await?
        .into_iter()
        .map(|pull| {
            Ok(Pull {
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
                created_at: pull
                    .created_at
                    .map(|v| OffsetDateTime::from_unix_timestamp(v.timestamp()))
                    .transpose()?,
                closed_at: pull
                    .closed_at
                    .map(|v| OffsetDateTime::from_unix_timestamp(v.timestamp()))
                    .transpose()?,
                merged_at: pull
                    .merged_at
                    .map(|v| OffsetDateTime::from_unix_timestamp(v.timestamp()))
                    .transpose()?,
                merge_commit_sha: pull.merge_commit_sha,
            })
        })
        .collect::<Result<_>>()?;

    to_xml(Pulls { pull })
}
