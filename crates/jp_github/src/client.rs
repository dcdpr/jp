use std::sync::{Arc, Mutex, OnceLock};

use reqwest::{
    Client,
    header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, USER_AGENT},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    Error, GitHubError, Page, Result, StatusCode,
    handlers::{CurrentHandler, IssuesHandler, PullsHandler, ReposHandler, SearchHandler},
};

#[derive(Clone)]
pub struct Octocrab {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub(crate) client: Client,
    pub(crate) api_base: String,
    pub(crate) graphql_url: String,
}

pub struct OctocrabBuilder {
    token: Option<String>,
}

impl Octocrab {
    #[must_use]
    pub fn builder() -> OctocrabBuilder {
        OctocrabBuilder { token: None }
    }

    #[must_use]
    pub fn current(&self) -> CurrentHandler {
        CurrentHandler {
            client: self.clone(),
        }
    }

    #[must_use]
    pub fn issues(&self, owner: impl Into<String>, repo: impl Into<String>) -> IssuesHandler {
        IssuesHandler {
            client: self.clone(),
            owner: owner.into(),
            repo: repo.into(),
        }
    }

    #[must_use]
    pub fn pulls(&self, owner: impl Into<String>, repo: impl Into<String>) -> PullsHandler {
        PullsHandler {
            client: self.clone(),
            owner: owner.into(),
            repo: repo.into(),
        }
    }

    #[must_use]
    pub fn repos(&self, owner: impl Into<String>, repo: impl Into<String>) -> ReposHandler {
        ReposHandler {
            client: self.clone(),
            owner: owner.into(),
            repo: repo.into(),
        }
    }

    #[must_use]
    pub fn search(&self) -> SearchHandler {
        SearchHandler {
            client: self.clone(),
        }
    }

    pub async fn graphql<T: DeserializeOwned>(&self, body: &Value) -> Result<T> {
        let request = self.inner.client.post(&self.inner.graphql_url).json(body);
        self.send_json(request).await
    }

    #[allow(clippy::unused_async)] // Keep async for octocrab API compatibility at callsites.
    pub async fn all_pages<T>(&self, page: Page<T>) -> Result<Vec<T>> {
        Ok(page.items)
    }

    pub(crate) async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(String, String)],
    ) -> Result<T> {
        let request = self
            .inner
            .client
            .get(format!("{}{}", self.inner.api_base, path))
            .query(query);

        self.send_json(request).await
    }

    pub(crate) async fn post_json<T: DeserializeOwned, B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let request = self
            .inner
            .client
            .post(format!("{}{}", self.inner.api_base, path))
            .json(body);

        self.send_json(request).await
    }

    pub(crate) async fn get_paginated<T: DeserializeOwned>(
        &self,
        path: &str,
        mut query: Vec<(String, String)>,
        per_page: u8,
    ) -> Result<Vec<T>> {
        let mut page = 1_u64;
        let mut out = vec![];

        loop {
            query.retain(|(key, _)| key != "page" && key != "per_page");
            query.push(("per_page".to_owned(), per_page.to_string()));
            query.push(("page".to_owned(), page.to_string()));

            let items: Vec<T> = self.get_json(path, &query).await?;
            let count = items.len();
            out.extend(items);

            if count == 0 || count < usize::from(per_page) {
                break;
            }

            page += 1;
        }

        Ok(out)
    }

    pub(crate) async fn get_search_paginated<T: DeserializeOwned>(
        &self,
        path: &str,
        mut query: Vec<(String, String)>,
        per_page: u8,
    ) -> Result<Vec<T>> {
        #[derive(serde::Deserialize)]
        struct SearchResponse<T> {
            items: Vec<T>,
        }

        let mut page = 1_u64;
        let mut out = vec![];

        loop {
            query.retain(|(key, _)| key != "page" && key != "per_page");
            query.push(("per_page".to_owned(), per_page.to_string()));
            query.push(("page".to_owned(), page.to_string()));

            let response: SearchResponse<T> = self.get_json(path, &query).await?;
            let count = response.items.len();
            out.extend(response.items);

            if count == 0 || count < usize::from(per_page) {
                break;
            }

            page += 1;
        }

        Ok(out)
    }

    async fn send_json<T: DeserializeOwned>(&self, request: reqwest::RequestBuilder) -> Result<T> {
        let response = request.send().await?;
        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            let message = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|value| {
                    value
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| format!("request failed with status {}", status.as_u16()));

            return Err(Error::GitHub {
                source: GitHubError {
                    status_code: StatusCode::new(status.as_u16()),
                    message,
                },
                body: Some(body),
            });
        }

        serde_json::from_str(&body).map_err(Into::into)
    }

    #[cfg(test)]
    pub(crate) fn with_base_url(base_url: &str, token: Option<&str>) -> Self {
        let client = build_http_client(token).expect("test client to build");

        Self {
            inner: Arc::new(Inner {
                client,
                api_base: base_url.to_owned(),
                graphql_url: format!("{base_url}/graphql"),
            }),
        }
    }
}

impl OctocrabBuilder {
    #[must_use]
    pub fn personal_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    pub fn build(self) -> Result<Octocrab> {
        let client = build_http_client(self.token.as_deref())?;

        Ok(Octocrab {
            inner: Arc::new(Inner {
                client,
                api_base: "https://api.github.com".to_owned(),
                graphql_url: "https://api.github.com/graphql".to_owned(),
            }),
        })
    }
}

fn build_http_client(token: Option<&str>) -> Result<Client> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("jp-github"));
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/vnd.github+json"),
    );
    headers.insert(
        HeaderName::from_static("x-github-api-version"),
        HeaderValue::from_static("2022-11-28"),
    );

    if let Some(token) = token {
        let token = format!("Bearer {token}");
        headers.insert(AUTHORIZATION, HeaderValue::from_str(&token)?);
    }

    Client::builder()
        .default_headers(headers)
        .build()
        .map_err(|error| Error::Build(format!("{error:#}")))
}

static INSTANCE: OnceLock<Mutex<Option<Octocrab>>> = OnceLock::new();

pub fn initialise(client: Octocrab) {
    let mutex = INSTANCE.get_or_init(|| Mutex::new(None));
    if let Ok(mut lock) = mutex.lock() {
        *lock = Some(client);
    }
}

#[must_use]
/// Returns the globally initialized GitHub client.
///
/// # Panics
///
/// Panics if [`initialise`] has not been called successfully.
pub fn instance() -> Octocrab {
    let mutex = INSTANCE.get_or_init(|| Mutex::new(None));
    let lock = mutex.lock().ok();

    match lock.and_then(|guard| guard.as_ref().cloned()) {
        Some(client) => client,
        None => panic!("jp_github client not initialized"),
    }
}
