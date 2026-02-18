use serde::Serialize;
use serde_json::Value;

use crate::{Octocrab, Page, Result, models, params};

pub struct CurrentHandler {
    pub(crate) client: Octocrab,
}

impl CurrentHandler {
    pub async fn user(&self) -> Result<models::User> {
        self.client.get_json("/user", &[]).await
    }
}

pub struct IssuesHandler {
    pub(crate) client: Octocrab,
    pub(crate) owner: String,
    pub(crate) repo: String,
}

impl IssuesHandler {
    pub async fn get(&self, number: u64) -> Result<models::issues::Issue> {
        self.client
            .get_json(
                &format!("/repos/{}/{}/issues/{number}", self.owner, self.repo),
                &[],
            )
            .await
    }

    #[must_use]
    pub fn list(&self) -> IssueListBuilder {
        IssueListBuilder {
            client: self.client.clone(),
            owner: self.owner.clone(),
            repo: self.repo.clone(),
            per_page: 30,
        }
    }

    #[must_use]
    pub fn create(&self, title: &str) -> IssueCreateBuilder {
        IssueCreateBuilder {
            client: self.client.clone(),
            owner: self.owner.clone(),
            repo: self.repo.clone(),
            title: title.to_owned(),
            body: None,
            labels: None,
            assignees: None,
        }
    }

    #[must_use]
    pub fn list_labels_for_repo(&self) -> RepoLabelsListBuilder {
        RepoLabelsListBuilder {
            client: self.client.clone(),
            owner: self.owner.clone(),
            repo: self.repo.clone(),
            per_page: 100,
        }
    }
}

pub struct IssueListBuilder {
    pub(crate) client: Octocrab,
    pub(crate) owner: String,
    pub(crate) repo: String,
    pub(crate) per_page: u8,
}

impl IssueListBuilder {
    #[must_use]
    pub const fn per_page(mut self, per_page: u8) -> Self {
        self.per_page = per_page;
        self
    }

    pub async fn send(self) -> Result<Page<models::issues::Issue>> {
        let items = self
            .client
            .get_paginated(
                &format!("/repos/{}/{}/issues", self.owner, self.repo),
                vec![],
                self.per_page,
            )
            .await?;

        Ok(Page::new(items))
    }
}

pub struct IssueCreateBuilder {
    pub(crate) client: Octocrab,
    pub(crate) owner: String,
    pub(crate) repo: String,
    pub(crate) title: String,
    pub(crate) body: Option<String>,
    pub(crate) labels: Option<Vec<String>>,
    pub(crate) assignees: Option<Vec<String>>,
}

impl IssueCreateBuilder {
    #[must_use]
    pub fn body(mut self, body: &str) -> Self {
        self.body = Some(body.to_owned());
        self
    }

    #[must_use]
    pub fn labels(mut self, labels: Option<Vec<String>>) -> Self {
        self.labels = labels;
        self
    }

    #[must_use]
    pub fn assignees(mut self, assignees: Option<Vec<String>>) -> Self {
        self.assignees = assignees;
        self
    }

    pub async fn send(self) -> Result<models::issues::Issue> {
        #[derive(Serialize)]
        struct CreateIssueBody {
            title: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            body: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            labels: Option<Vec<String>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            assignees: Option<Vec<String>>,
        }

        let body = CreateIssueBody {
            title: self.title,
            body: self.body,
            labels: self.labels,
            assignees: self.assignees,
        };

        self.client
            .post_json(
                &format!("/repos/{}/{}/issues", self.owner, self.repo),
                &body,
            )
            .await
    }
}

pub struct RepoLabelsListBuilder {
    pub(crate) client: Octocrab,
    pub(crate) owner: String,
    pub(crate) repo: String,
    pub(crate) per_page: u8,
}

impl RepoLabelsListBuilder {
    pub async fn send(self) -> Result<Page<models::Label>> {
        let items = self
            .client
            .get_paginated(
                &format!("/repos/{}/{}/labels", self.owner, self.repo),
                vec![],
                self.per_page,
            )
            .await?;

        Ok(Page::new(items))
    }
}

pub struct PullsHandler {
    pub(crate) client: Octocrab,
    pub(crate) owner: String,
    pub(crate) repo: String,
}

impl PullsHandler {
    pub async fn get(&self, number: u64) -> Result<models::pulls::PullRequest> {
        self.client
            .get_json(
                &format!("/repos/{}/{}/pulls/{number}", self.owner, self.repo),
                &[],
            )
            .await
    }

    pub async fn list_files(&self, number: u64) -> Result<Page<models::repos::DiffEntry>> {
        let items = self
            .client
            .get_paginated(
                &format!("/repos/{}/{}/pulls/{number}/files", self.owner, self.repo),
                vec![],
                100,
            )
            .await?;

        Ok(Page::new(items))
    }

    #[must_use]
    pub fn list(&self) -> PullListBuilder {
        PullListBuilder {
            client: self.client.clone(),
            owner: self.owner.clone(),
            repo: self.repo.clone(),
            state: params::State::Open,
            per_page: 30,
        }
    }
}

pub struct PullListBuilder {
    pub(crate) client: Octocrab,
    pub(crate) owner: String,
    pub(crate) repo: String,
    pub(crate) state: params::State,
    pub(crate) per_page: u8,
}

impl PullListBuilder {
    #[must_use]
    pub const fn state(mut self, state: params::State) -> Self {
        self.state = state;
        self
    }

    #[must_use]
    pub const fn per_page(mut self, per_page: u8) -> Self {
        self.per_page = per_page;
        self
    }

    pub async fn send(self) -> Result<Page<models::pulls::PullRequest>> {
        let query = vec![("state".to_owned(), self.state.as_str().to_owned())];
        let items = self
            .client
            .get_paginated(
                &format!("/repos/{}/{}/pulls", self.owner, self.repo),
                query,
                self.per_page,
            )
            .await?;

        Ok(Page::new(items))
    }
}

pub struct ReposHandler {
    pub(crate) client: Octocrab,
    pub(crate) owner: String,
    pub(crate) repo: String,
}

impl ReposHandler {
    #[must_use]
    pub fn get_content(&self) -> RepoContentBuilder {
        RepoContentBuilder {
            client: self.client.clone(),
            owner: self.owner.clone(),
            repo: self.repo.clone(),
            path: String::new(),
            reference: None,
        }
    }

    #[must_use]
    pub fn list_collaborators(&self) -> RepoCollaboratorListBuilder {
        RepoCollaboratorListBuilder {
            client: self.client.clone(),
            owner: self.owner.clone(),
            repo: self.repo.clone(),
            per_page: 100,
        }
    }
}

pub struct RepoContentBuilder {
    pub(crate) client: Octocrab,
    pub(crate) owner: String,
    pub(crate) repo: String,
    pub(crate) path: String,
    pub(crate) reference: Option<String>,
}

impl RepoContentBuilder {
    #[must_use]
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    #[must_use]
    pub fn r#ref(mut self, reference: impl Into<String>) -> Self {
        self.reference = Some(reference.into());
        self
    }

    pub async fn send(self) -> Result<models::repos::ContentItems> {
        let path = if self.path.is_empty() {
            format!("/repos/{}/{}/contents", self.owner, self.repo)
        } else {
            format!("/repos/{}/{}/contents/{}", self.owner, self.repo, self.path)
        };

        let mut query = vec![];
        if let Some(reference) = self.reference {
            query.push(("ref".to_owned(), reference));
        }

        let value: Value = self.client.get_json(&path, &query).await?;

        let items = match value {
            Value::Array(array) => array,
            value => vec![value],
        }
        .into_iter()
        .map(serde_json::from_value)
        .collect::<std::result::Result<Vec<models::repos::ContentItem>, _>>()?;

        Ok(models::repos::ContentItems::new(items))
    }
}

pub struct RepoCollaboratorListBuilder {
    pub(crate) client: Octocrab,
    pub(crate) owner: String,
    pub(crate) repo: String,
    pub(crate) per_page: u8,
}

impl RepoCollaboratorListBuilder {
    pub async fn send(self) -> Result<Page<models::repos::Collaborator>> {
        let users: Vec<models::User> = self
            .client
            .get_paginated(
                &format!("/repos/{}/{}/collaborators", self.owner, self.repo),
                vec![],
                self.per_page,
            )
            .await?;

        let items = users
            .into_iter()
            .map(|author| models::repos::Collaborator { author })
            .collect();

        Ok(Page::new(items))
    }
}

pub struct SearchHandler {
    pub(crate) client: Octocrab,
}

impl SearchHandler {
    #[must_use]
    pub fn code(&self, query: &str) -> CodeSearchBuilder {
        CodeSearchBuilder {
            client: self.client.clone(),
            query: query.to_owned(),
            per_page: 100,
        }
    }
}

pub struct CodeSearchBuilder {
    pub(crate) client: Octocrab,
    pub(crate) query: String,
    pub(crate) per_page: u8,
}

impl CodeSearchBuilder {
    pub async fn send(self) -> Result<Page<models::search::CodeItem>> {
        let items = self
            .client
            .get_search_paginated(
                "/search/code",
                vec![("q".to_owned(), self.query)],
                self.per_page,
            )
            .await?;

        Ok(Page::new(items))
    }
}
