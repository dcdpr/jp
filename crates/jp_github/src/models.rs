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
        pub title: Option<String>,
        pub body: Option<String>,
        pub html_url: Option<Url>,
        pub labels: Option<Vec<Label>>,
        pub user: Option<User>,
        pub created_at: Option<DateTime<Utc>>,
        pub closed_at: Option<DateTime<Utc>>,
        pub merged_at: Option<DateTime<Utc>>,
        pub merge_commit_sha: Option<String>,
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
