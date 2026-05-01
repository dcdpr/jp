use serde::Serialize;
use serde_json::Value;

use crate::{Error, GitHubError, Octocrab, Page, Result, StatusCode, models, params};

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

    /// Fetch a pull request as a unified diff.
    ///
    /// Sends `Accept: application/vnd.github.diff` to the same endpoint as
    /// `get`, getting the diff text instead of JSON metadata.
    pub async fn diff(&self, number: u64) -> Result<String> {
        self.client
            .get_with_accept(
                &format!("/repos/{}/{}/pulls/{number}", self.owner, self.repo),
                "application/vnd.github.diff",
            )
            .await
    }

    /// Fetch every inline review comment on a pull request, including
    /// pending review comments authored by the current user.
    ///
    /// Uses GraphQL `pullRequest.reviewThreads` so each returned
    /// [`ReviewComment`] carries:
    ///
    /// - The thread-level [`Side`] in `side` / `start_side`.
    /// - GitHub's authoritative `outdated` flag
    ///   ([`ReviewComment::outdated`]).
    /// - File-line anchors (`line`, `start_line`, `original_line`,
    ///   `original_start_line`) directly from GraphQL, which are reliable
    ///   even for pending comments where REST returns null.
    ///
    /// Caps at the first 100 threads, each with up to 100 comments. Larger
    /// PRs are silently truncated for now; pagination can be added if it
    /// becomes a real limit.
    ///
    /// [`ReviewComment`]: models::pulls::ReviewComment
    /// [`Side`]: models::pulls::Side
    /// [`ReviewComment::outdated`]: models::pulls::ReviewComment::outdated
    #[allow(clippy::too_many_lines, reason = "linear walk over GraphQL JSON")]
    pub async fn fetch_review_comments(
        &self,
        number: u64,
    ) -> Result<Vec<models::pulls::ReviewComment>> {
        let query = indoc::indoc! {"
            query Reviews($owner: String!, $name: String!, $number: Int!) {
              repository(owner: $owner, name: $name) {
                pullRequest(number: $number) {
                  reviewThreads(first: 100) {
                    nodes {
                      isOutdated
                      diffSide
                      startDiffSide
                      comments(first: 100) {
                        nodes {
                          fullDatabaseId
                          path
                          outdated
                          line
                          startLine
                          originalLine
                          originalStartLine
                          body
                          createdAt
                          author { login }
                          replyTo { fullDatabaseId }
                          pullRequestReview { fullDatabaseId }
                        }
                      }
                    }
                  }
                }
              }
            }
        "};

        let body = serde_json::json!({
            "query": query,
            "variables": {
                "owner": self.owner,
                "name": self.repo,
                "number": number,
            },
        });

        let response: Value = self.client.graphql(&body).await?;

        let mut out = Vec::new();
        let threads = response
            .pointer("/data/repository/pullRequest/reviewThreads/nodes")
            .and_then(|v| v.as_array());

        for thread in threads.into_iter().flatten() {
            let thread_side = parse_diff_side(thread.get("diffSide"));
            let thread_start_side = parse_diff_side(thread.get("startDiffSide"));
            let thread_outdated = thread
                .get("isOutdated")
                .and_then(Value::as_bool)
                .unwrap_or(false);

            let comments = thread.pointer("/comments/nodes").and_then(|v| v.as_array());

            for comment in comments.into_iter().flatten() {
                let Some(id) = parse_full_database_id(comment.get("fullDatabaseId")) else {
                    continue;
                };
                let path = comment
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let body = comment
                    .get("body")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                // The thread-level `isOutdated` is canonical; the
                // per-comment `outdated` field should agree, but fall back
                // to the thread if it is somehow missing.
                let outdated = comment
                    .get("outdated")
                    .and_then(Value::as_bool)
                    .unwrap_or(thread_outdated);
                let line = comment.get("line").and_then(Value::as_u64);
                let start_line = comment.get("startLine").and_then(Value::as_u64);
                let original_line = comment.get("originalLine").and_then(Value::as_u64);
                let original_start_line = comment.get("originalStartLine").and_then(Value::as_u64);
                let created_at = comment
                    .get("createdAt")
                    .and_then(Value::as_str)
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&chrono::Utc));
                let user = comment
                    .pointer("/author/login")
                    .and_then(Value::as_str)
                    .map(|login| models::User {
                        login: login.to_owned(),
                    });
                let in_reply_to_id =
                    parse_full_database_id(comment.pointer("/replyTo/fullDatabaseId"));
                let pull_request_review_id =
                    parse_full_database_id(comment.pointer("/pullRequestReview/fullDatabaseId"));

                out.push(models::pulls::ReviewComment {
                    id,
                    pull_request_review_id,
                    path,
                    line,
                    side: thread_side,
                    start_line,
                    start_side: thread_start_side,
                    original_line,
                    original_side: thread_side,
                    original_start_line,
                    original_start_side: thread_start_side,
                    in_reply_to_id,
                    body,
                    user,
                    created_at,
                    outdated,
                });
            }
        }

        Ok(out)
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

    /// List all reviews on a pull request.
    pub async fn list_reviews(&self, number: u64) -> Result<Page<models::pulls::Review>> {
        let items = self
            .client
            .get_paginated(
                &format!("/repos/{}/{}/pulls/{number}/reviews", self.owner, self.repo),
                vec![],
                100,
            )
            .await?;

        Ok(Page::new(items))
    }

    /// Delete a pending review on a pull request.
    ///
    /// GitHub only permits deletion while the review is in `PENDING` state.
    pub async fn delete_review(&self, number: u64, review_id: u64) -> Result<()> {
        self.client
            .delete_no_content(&format!(
                "/repos/{}/{}/pulls/{number}/reviews/{review_id}",
                self.owner, self.repo,
            ))
            .await
    }

    /// Begin building a new review for a pull request.
    ///
    /// The default builder produces a `PENDING` (draft) review when sent.
    /// Call [`PullReviewCreateBuilder::event`] to submit immediately.
    #[must_use]
    pub fn create_review(&self, number: u64) -> PullReviewCreateBuilder {
        PullReviewCreateBuilder {
            client: self.client.clone(),
            owner: self.owner.clone(),
            repo: self.repo.clone(),
            number,
            commit_id: None,
            body: None,
            event: None,
            comments: vec![],
        }
    }

    /// Add an inline comment to an existing pending review.
    ///
    /// The GitHub REST API does not support appending comments to an existing
    /// pending review (creating a review is all-or-nothing). This uses the
    /// GraphQL `addPullRequestReviewThread` mutation, which does support it.
    ///
    /// `review_node_id` is the review's GraphQL `node_id` (NOT its integer
    /// `id`). When `start_line` is set, the comment is multi-line; the range
    /// is `[start_line, line]` on the chosen `side`.
    pub async fn add_review_thread(
        &self,
        review_node_id: &str,
        comment: &models::pulls::DraftReviewComment,
    ) -> Result<()> {
        let query = indoc::indoc! {"
            mutation AddThread($input: AddPullRequestReviewThreadInput!) {
              addPullRequestReviewThread(input: $input) {
                thread { id }
              }
            }
        "};

        let mut input = serde_json::Map::new();
        input.insert(
            "pullRequestReviewId".to_owned(),
            Value::String(review_node_id.to_owned()),
        );
        input.insert("path".to_owned(), Value::String(comment.path.clone()));
        input.insert("body".to_owned(), Value::String(comment.body.clone()));
        input.insert("line".to_owned(), Value::Number(comment.line.into()));
        if let Some(side) = comment.side {
            input.insert(
                "side".to_owned(),
                Value::String(side_to_str(side).to_owned()),
            );
        }
        if let Some(start_line) = comment.start_line {
            input.insert("startLine".to_owned(), Value::Number(start_line.into()));
            if let Some(start_side) = comment.start_side {
                input.insert(
                    "startSide".to_owned(),
                    Value::String(side_to_str(start_side).to_owned()),
                );
            }
        }

        let body = serde_json::json!({
            "query": query,
            "variables": { "input": Value::Object(input) },
        });

        let response: Value = self.client.graphql(&body).await?;
        if let Some(errors) = response.get("errors")
            && !errors.is_null()
        {
            let message = errors
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("unknown GraphQL error");

            return Err(Error::GitHub {
                source: GitHubError {
                    status_code: StatusCode::new(200),
                    message: format!("addPullRequestReviewThread: {message}"),
                },
                body: Some(errors.to_string()),
            });
        }

        Ok(())
    }
}

const fn side_to_str(side: models::pulls::Side) -> &'static str {
    match side {
        models::pulls::Side::Right => "RIGHT",
        models::pulls::Side::Left => "LEFT",
    }
}

/// Parse a GraphQL `DiffSide` enum value into [`models::pulls::Side`].
/// Anything other than the two known values yields `None`, which the
/// caller treats as "side unknown" and falls back to defaults at render.
fn parse_diff_side(v: Option<&Value>) -> Option<models::pulls::Side> {
    match v?.as_str()? {
        "RIGHT" => Some(models::pulls::Side::Right),
        "LEFT" => Some(models::pulls::Side::Left),
        _ => None,
    }
}

/// Parse a GraphQL `BigInt` (returned as a JSON string) into a `u64`.
/// Negative or out-of-range values yield `None`.
fn parse_full_database_id(v: Option<&Value>) -> Option<u64> {
    v?.as_str()?.parse().ok()
}

pub struct PullReviewCreateBuilder {
    pub(crate) client: Octocrab,
    pub(crate) owner: String,
    pub(crate) repo: String,
    pub(crate) number: u64,
    pub(crate) commit_id: Option<String>,
    pub(crate) body: Option<String>,
    pub(crate) event: Option<String>,
    pub(crate) comments: Vec<models::pulls::DraftReviewComment>,
}

impl PullReviewCreateBuilder {
    #[must_use]
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = Some(body.into());
        self
    }

    #[must_use]
    pub fn commit_id(mut self, commit_id: impl Into<String>) -> Self {
        self.commit_id = Some(commit_id.into());
        self
    }

    /// Set an explicit `event`. Omit this to leave the review as a draft
    /// (GitHub treats a missing event as `PENDING`).
    #[must_use]
    pub fn event(mut self, event: impl Into<String>) -> Self {
        self.event = Some(event.into());
        self
    }

    #[must_use]
    pub fn comments(mut self, comments: Vec<models::pulls::DraftReviewComment>) -> Self {
        self.comments = comments;
        self
    }

    pub async fn send(self) -> Result<models::pulls::Review> {
        #[derive(Serialize)]
        struct CreateReviewBody {
            #[serde(skip_serializing_if = "Option::is_none")]
            commit_id: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            body: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            event: Option<String>,
            comments: Vec<models::pulls::DraftReviewComment>,
        }

        let payload = CreateReviewBody {
            commit_id: self.commit_id,
            body: self.body,
            event: self.event,
            comments: self.comments,
        };

        self.client
            .post_json(
                &format!(
                    "/repos/{}/{}/pulls/{}/reviews",
                    self.owner, self.repo, self.number
                ),
                &payload,
            )
            .await
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
