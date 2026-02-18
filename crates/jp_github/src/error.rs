pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("GitHub API error: {source}")]
    GitHub {
        source: GitHubError,
        body: Option<String>,
    },

    #[error("HTTP client error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("invalid HTTP header value: {0}")]
    InvalidHeader(#[from] reqwest::header::InvalidHeaderValue),

    #[error("failed to build github client: {0}")]
    Build(String),

    #[error("github client not initialized")]
    NotInitialized,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{} ({})", message, status_code.as_u16())]
pub struct GitHubError {
    pub status_code: StatusCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct StatusCode(u16);

impl StatusCode {
    #[must_use]
    pub const fn as_u16(self) -> u16 {
        self.0
    }

    #[must_use]
    pub(crate) const fn new(value: u16) -> Self {
        Self(value)
    }
}

impl PartialEq<u16> for StatusCode {
    fn eq(&self, other: &u16) -> bool {
        self.0 == *other
    }
}
