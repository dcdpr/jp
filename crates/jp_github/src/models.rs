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
    /// unreliable for pending review comments (often null even when the
    /// comment is still anchored), and it has no equivalent of the
    /// authoritative [`Self::outdated`] flag.
    ///
    /// The `original_*` fields preserve the comment's anchor as it was at
    /// creation time. GraphQL does not expose an `original_side`
    /// equivalent, so `original_side` and `original_start_side` are
    /// populated from the current thread side as an approximation — good
    /// enough for rendering, since side rarely changes for a thread.
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
        /// from `pullRequest.reviewThreads.isOutdated`. The same field
        /// GitHub's web UI uses to collapse outdated threads.
        pub outdated: bool,
    }

    /// Inline comment payload for a draft review.
    ///
    /// `line` is 1-based and refers to a line in the file on the chosen
    /// `side` of the diff (RIGHT = the new revision, LEFT = the old).
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

pub mod search {
    use super::{Deserialize, Serialize};

    #[derive(Debug, Clone, Deserialize, Serialize)]
    pub struct CodeItem {
        pub path: String,
        pub sha: String,
    }
}
