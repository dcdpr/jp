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

/// A release to create.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NewRelease {
    /// The tag the release is of.
    /// It has to exist: GitHub creates one that doesn't, at the tip of the
    /// default branch.
    pub tag_name: String,
    pub name: String,
    pub body: String,
    pub draft: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub make_latest: Option<MakeLatest>,
}

/// Changes to a release: a field left `None` stays as it is.
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

/// Whether a published release becomes the repository's latest, the one
/// `releases/latest` serves.
///
/// GitHub makes a newly published release the latest unless told otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MakeLatest {
    True,
    False,
}
