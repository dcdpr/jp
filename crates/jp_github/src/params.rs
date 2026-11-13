use serde::Serialize;

#[derive(Debug, Clone, Copy)]
pub enum State {
    Open,
    Closed,
    All,
}

impl State {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::All => "all",
        }
    }
}

/// Request body for creating a GitHub Release (`POST
/// /repos/{owner}/{repo}/releases`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NewRelease {
    pub tag_name: String,
    pub name: String,
    pub body: String,
    pub draft: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub make_latest: Option<MakeLatest>,
}

/// Request body for updating a GitHub Release (`PATCH
/// /repos/{owner}/{repo}/releases/{id}`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReleaseUpdate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub make_latest: Option<MakeLatest>,
}

/// Whether a release should be marked as the repository's `latest` release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MakeLatest {
    True,
    False,
}
