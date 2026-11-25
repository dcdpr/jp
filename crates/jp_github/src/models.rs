use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct User {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Label {
    pub name: String,
    pub description: Option<String>,
}

pub mod issues {
    use super::{DateTime, Deserialize, Label, Serialize, Url, User, Utc};

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct PullRequestLink {
        pub html_url: Url,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct Issue {
        pub number: u64,
        pub title: String,
        pub body: Option<String>,
        pub html_url: Url,
        pub labels: Vec<Label>,
        pub user: User,
        pub created_at: DateTime<Utc>,
        pub closed_at: Option<DateTime<Utc>>,
        pub pull_request: Option<PullRequestLink>,
        /// Total number of conversation comments on this issue or pull request.
        /// The list endpoint and individual issue endpoint both expose this —
        /// the field is missing from some payloads we don't care about (e.g.
        /// event payloads), so it's defaulted to 0.
        #[serde(default)]
        pub comments: u64,
    }

    /// A conversation comment on an issue or pull request.
    ///
    /// Sourced from `/repos/{owner}/{repo}/issues/{number}/comments`, which
    /// also serves PR conversation comments (PRs are issues for this endpoint).
    /// Inline PR review comments live on a different endpoint and are modeled
    /// separately by [`super::pulls::ReviewComment`].
    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct Comment {
        pub id: u64,
        pub user: User,
        pub body: Option<String>,
        pub html_url: Url,
        pub created_at: DateTime<Utc>,
        pub updated_at: Option<DateTime<Utc>>,
    }
}

pub mod pulls {
    use super::{DateTime, Deserialize, Label, Serialize, Url, User, Utc};

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct PullRequest {
        pub number: u64,
        #[serde(default)]
        pub node_id: String,
        pub title: Option<String>,
        pub body: Option<String>,
        pub html_url: Option<Url>,
        pub labels: Option<Vec<Label>>,
        pub user: Option<User>,
        pub created_at: Option<DateTime<Utc>>,
        pub closed_at: Option<DateTime<Utc>>,
        pub merged_at: Option<DateTime<Utc>>,
        pub merge_commit_sha: Option<String>,
        pub head: Option<GitRef>,
        pub base: Option<GitRef>,
        /// Total number of conversation comments on this pull request, shared
        /// with the issue-comments endpoint.
        /// Inline review comments are counted separately by GitHub and not
        /// reflected here.
        /// Defaulted to 0 because the field is absent from some payloads we
        /// don't care about (e.g. webhook events).
        #[serde(default)]
        pub comments: u64,
        /// Total number of files changed in this pull request.
        /// Exposed by the PR detail endpoint; defaulted to 0 because the field
        /// is absent from list-style payloads.
        #[serde(default)]
        pub changed_files: u64,
    }

    /// A reference to a git object (branch tip or base).
    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct GitRef {
        pub sha: String,
        #[serde(rename = "ref", default)]
        pub ref_: Option<String>,
    }

    /// Which side of a unified diff a review comment refers to.
    ///
    /// `Right` is the destination (new) revision, `Left` the source (old).
    #[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
    #[serde(rename_all = "UPPERCASE")]
    pub enum Side {
        Right,
        Left,
    }

    /// Lifecycle state of a pull request review.
    #[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
    #[serde(rename_all = "SCREAMING_SNAKE_CASE")]
    pub enum ReviewState {
        Pending,
        Commented,
        Approved,
        ChangesRequested,
        Dismissed,
        #[serde(other)]
        Unknown,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct Review {
        pub id: u64,
        #[serde(default)]
        pub node_id: String,
        pub state: ReviewState,
        pub body: Option<String>,
        pub html_url: Option<Url>,
        pub user: Option<User>,
    }

    /// An inline review comment, as returned by GitHub's GraphQL API.
    ///
    /// Sourced from `pullRequest.reviewThreads.comments`, with thread-level
    /// fields (`diffSide`, `startDiffSide`) inherited into the per-comment
    /// `side` / `start_side` so callers can treat each comment as
    /// self-contained.
    ///
    /// REST is intentionally not used: its `line` and `position` fields are
    /// unreliable for pending review comments (often null even when the comment
    /// is still anchored), and it has no equivalent of the authoritative
    /// [`Self::outdated`] flag.
    ///
    /// The `original_*` fields preserve the comment's anchor as it was at
    /// creation time.
    /// GraphQL does not expose an `original_side` equivalent, so
    /// `original_side` and `original_start_side` are populated from the current
    /// thread side as an approximation — good enough for rendering, since side
    /// rarely changes for a thread.
    #[derive(Debug, Clone)]
    pub struct ReviewComment {
        pub id: u64,
        pub pull_request_review_id: Option<u64>,
        pub path: String,
        pub line: Option<u64>,
        pub side: Option<Side>,
        pub start_line: Option<u64>,
        pub start_side: Option<Side>,
        pub original_line: Option<u64>,
        pub original_side: Option<Side>,
        pub original_start_line: Option<u64>,
        pub original_start_side: Option<Side>,
        pub in_reply_to_id: Option<u64>,
        pub body: String,
        pub user: Option<User>,
        pub created_at: Option<DateTime<Utc>>,
        /// GitHub's authoritative "outdated" flag for this comment, sourced
        /// from `pullRequest.reviewThreads.isOutdated`.
        /// The same field GitHub's web UI uses to collapse outdated threads.
        pub outdated: bool,
    }

    /// Inline comment payload for a draft review.
    ///
    /// `line` is 1-based and refers to a line in the file on the chosen `side`
    /// of the diff (RIGHT = the new revision, LEFT = the old).
    /// Pair with `start_line` (and optional `start_side`) to anchor a
    /// multi-line comment.
    #[derive(Debug, Clone, Serialize)]
    pub struct DraftReviewComment {
        pub path: String,
        pub body: String,
        pub line: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub side: Option<Side>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub start_line: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub start_side: Option<Side>,
    }

    /// The thread created by an `addPullRequestReviewThread` mutation.
    #[derive(Debug, Clone)]
    pub struct CreatedReviewThread {
        /// GraphQL node ID of the created thread.
        pub id: String,

        /// The diff line the thread resolved to in the PR's current diff.
        ///
        /// `None` means GitHub accepted the mutation but could not place the
        /// thread in the diff it renders (the anchor lies outside the diff's
        /// hunks): the thread exists, yet the review UI never displays it.
        pub line: Option<u64>,

        /// GitHub's authoritative "outdated" flag for the thread.
        pub is_outdated: bool,
    }
}

pub mod repos {
    use super::{Deserialize, Serialize, User};

    #[derive(Debug, Clone, Copy, Deserialize, Serialize)]
    #[serde(rename_all = "lowercase")]
    pub enum DiffEntryStatus {
        Added,
        Removed,
        Modified,
        Renamed,
        Copied,
        Changed,
        Unchanged,
        #[serde(other)]
        Unknown,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct DiffEntry {
        pub filename: String,
        pub status: DiffEntryStatus,
        pub additions: u64,
        pub deletions: u64,
        pub changes: u64,
        pub previous_filename: Option<String>,
        pub patch: Option<String>,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct ContentItem {
        pub path: String,
        #[serde(rename = "type")]
        pub r#type: String,
        pub content: Option<String>,
        pub encoding: Option<String>,
    }

    #[derive(Debug, Clone)]
    pub struct ContentItems {
        items: Vec<ContentItem>,
    }

    impl ContentItems {
        #[must_use]
        pub fn new(items: Vec<ContentItem>) -> Self {
            Self { items }
        }

        #[must_use]
        pub fn take_items(self) -> Vec<ContentItem> {
            self.items
        }
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct Collaborator {
        pub author: User,
    }
}

pub mod commits {
    use super::{DateTime, Deserialize, Serialize, User, Utc, repos::DiffEntry};

    /// A commit, as returned by the PR commit-list and single-commit endpoints.
    ///
    /// `stats` and `files` are populated only by the single-commit endpoint
    /// (`GET /repos/{owner}/{repo}/commits/{ref}`).
    /// The PR commit-list endpoint omits them, so both are `Option`.
    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct Commit {
        pub sha: String,
        pub commit: CommitDetails,
        /// The GitHub account linked to the commit author's email, when one
        /// exists.
        /// Null for commits whose author isn't a known GitHub user.
        pub author: Option<User>,
        #[serde(default)]
        pub stats: Option<CommitStats>,
        #[serde(default)]
        pub files: Option<Vec<DiffEntry>>,
    }

    /// The git-level commit payload (message and authorship), nested under
    /// `commit` in the API response.
    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct CommitDetails {
        pub message: String,
        pub author: Option<CommitAuthor>,
    }

    /// Git authorship recorded in the commit itself, distinct from the GitHub
    /// account in [`Commit::author`].
    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct CommitAuthor {
        pub name: Option<String>,
        pub email: Option<String>,
        pub date: Option<DateTime<Utc>>,
    }

    /// Aggregate line stats for a commit, present on the single-commit
    /// endpoint.
    #[derive(Debug, Clone, Copy, Deserialize, Serialize)]
    pub struct CommitStats {
        pub additions: u64,
        pub deletions: u64,
        pub total: u64,
    }
}

pub mod checks {
    use super::{DateTime, Deserialize, Serialize, Url, Utc};

    /// A page of check runs for a commit, returned by `GET
    /// /repos/{owner}/{repo}/commits/{ref}/check-runs`.
    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct CheckRuns {
        /// Total number of check runs across all pages.
        pub total_count: u64,
        pub check_runs: Vec<CheckRun>,
    }

    /// A check run reported on a commit, either a GitHub Actions job or an
    /// external GitHub App check.
    ///
    /// For GitHub Actions jobs, `id` is also the job ID passed to
    /// [`ReposHandler::job_logs`].
    ///
    /// [`ReposHandler::job_logs`]: crate::ReposHandler::job_logs
    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct CheckRun {
        pub id: u64,
        pub name: String,
        pub status: CheckStatus,
        /// Populated once `status` is [`CheckStatus::Completed`].
        pub conclusion: Option<CheckConclusion>,
        pub started_at: Option<DateTime<Utc>>,
        pub completed_at: Option<DateTime<Utc>>,
        pub html_url: Option<Url>,
        pub output: CheckRunOutput,
        /// The GitHub App that created the check run.
        pub app: Option<CheckApp>,
    }

    impl CheckRun {
        /// Return whether this check run was created by GitHub Actions (and
        /// therefore has an Actions job log).
        // qual:api
        // Called from `.config/jp/tools`, which rustqual doesn't analyse
        // (T-0yh5j3r item 17).
        #[must_use]
        pub fn is_actions_job(&self) -> bool {
            self.app
                .as_ref()
                .is_some_and(|app| app.slug == "github-actions")
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
    #[serde(rename_all = "snake_case")]
    pub enum CheckStatus {
        Queued,
        InProgress,
        Completed,
        Waiting,
        Requested,
        Pending,
        #[serde(other)]
        Unknown,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
    #[serde(rename_all = "snake_case")]
    pub enum CheckConclusion {
        Success,
        Failure,
        Neutral,
        Cancelled,
        Skipped,
        TimedOut,
        ActionRequired,
        Stale,
        #[serde(other)]
        Unknown,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct CheckRunOutput {
        pub title: Option<String>,
        pub summary: Option<String>,
        /// Number of annotations attached to the check run, fetched separately
        /// via [`ReposHandler::list_check_run_annotations`].
        ///
        /// [`ReposHandler::list_check_run_annotations`]: crate::ReposHandler::list_check_run_annotations
        #[serde(default)]
        pub annotations_count: u64,
    }

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct CheckApp {
        pub slug: String,
    }

    /// A diagnostic annotation attached to a check run at a file and line range
    /// (for example, a compiler error extracted by a problem matcher).
    ///
    /// GitHub's schema permits `annotation_level` and `message` to be `null`.
    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct Annotation {
        /// Repository-relative file path.
        pub path: String,
        pub start_line: u64,
        pub end_line: u64,
        pub annotation_level: Option<AnnotationLevel>,
        pub title: Option<String>,
        pub message: Option<String>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
    #[serde(rename_all = "snake_case")]
    pub enum AnnotationLevel {
        Notice,
        Warning,
        Failure,
        #[serde(other)]
        Unknown,
    }

    /// Combined commit statuses returned by `GET
    /// /repos/{owner}/{repo}/commits/{ref}/status`.
    ///
    /// Commit statuses are distinct from check runs: workflows and external
    /// services publish them via the statuses API, and they do not appear in
    /// the check-runs endpoint.
    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct CombinedStatus {
        /// Total number of statuses across all pages.
        pub total_count: u64,
        /// Latest status for each context.
        pub statuses: Vec<Status>,
    }

    /// The latest status reported for a given `context` on a commit.
    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct Status {
        pub context: String,
        pub state: StatusState,
        pub description: Option<String>,
        pub target_url: Option<String>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
    #[serde(rename_all = "snake_case")]
    pub enum StatusState {
        Error,
        Failure,
        Pending,
        Success,
        #[serde(other)]
        Unknown,
    }
}

pub mod search {
    use super::{Deserialize, Serialize};

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct CodeItem {
        pub path: String,
        pub sha: String,
    }
}
