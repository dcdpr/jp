//! Forge detection, change-request extraction from commit messages, and credit
//! link formatting.
//!
//! Each fragment and highlight is credited to the change requests that modified
//! it on the default branch, or to the commit itself when the commit message
//! does not reference a change request:
//!
//! | Forge    | Commit message pattern                                            | Label  | Target URL                                      |
//! | -------- | ----------------------------------------------------------------- | ------ | ----------------------------------------------- |
//! | `github` | Trailing `(#123)` on squash subject, or `Merge pull request #123` | `#123` | `https://<host>/<owner>/<repo>/pull/123`        |
//! | `gitlab` | Trailing `See merge request <project>!123` line                   | `!123` | `https://<host>/<project>/-/merge_requests/123` |
//! | `none`   | None                                                              |        |                                                 |
//!
//! Commits are labeled with their 7-character short hash and link to
//! `/commit/<hash>` on GitHub, `/-/commit/<hash>` on GitLab, or no URL when the
//! forge is `none`.

use std::{fmt, num::NonZeroU64, str::FromStr};

use serde::Serialize;

const SHORT_HASH: usize = 7;

/// Source code hosting forge where change requests are reviewed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forge {
    /// GitHub (`github.com` or self-hosted GitHub Enterprise): pull requests.
    GitHub,

    /// GitLab (`gitlab.com` or self-hosted GitLab): merge requests.
    GitLab,

    /// No forge: all credits are plain commit hashes without links.
    None,
}

impl Forge {
    pub const ALL: [Self; 3] = [Self::GitHub, Self::GitLab, Self::None];

    /// Configuration identifier for the forge (`github`, `gitlab`, or `none`).
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::GitHub => "github",
            Self::GitLab => "gitlab",
            Self::None => "none",
        }
    }

    /// Infer the forge from a repository URL's hostname (`github.com` ->
    /// [`Forge::GitHub`], `gitlab.com` -> [`Forge::GitLab`], anything else ->
    /// [`Forge::None`]).
    ///
    /// Self-hosted GitHub or GitLab instances cannot be inferred from the
    /// hostname and must configure the forge explicitly.
    #[must_use]
    pub fn infer(address: Option<&str>) -> Self {
        let host = address.and_then(host).map(str::to_ascii_lowercase);

        match host.as_deref() {
            Some("github.com") => Self::GitHub,
            Some("gitlab.com") => Self::GitLab,
            _ => Self::None,
        }
    }

    /// Extract a change request number from `message` using this forge's merge
    /// or squash commit convention.
    #[must_use]
    pub fn change_request(self, message: &str) -> Option<NonZeroU64> {
        match self {
            Self::GitHub => github_change_request(message),
            Self::GitLab => gitlab_change_request(message),
            Self::None => None,
        }
    }
}

impl fmt::Display for Forge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for Forge {
    type Err = UnknownForge;

    fn from_str(id: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|forge| forge.id() == id)
            .ok_or_else(|| UnknownForge(id.to_owned()))
    }
}

/// Error returned when parsing an unrecognized [`Forge`] identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownForge(pub String);

impl fmt::Display for UnknownForge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` is not a forge; use `github`, `gitlab`, or `none`",
            self.0
        )
    }
}

impl std::error::Error for UnknownForge {}

/// Forge and base repository URL used to format [`Credit`] labels and links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    forge: Forge,

    /// Base repository web URL (without trailing `/`), or `None` when `forge`
    /// is [`Forge::None`].
    address: Option<String>,
}

impl Project {
    /// Create a [`Project`] for `forge` with the given web `address` (stripping
    /// any trailing `/`).
    ///
    /// When `forge` is [`Forge::None`], `address` is ignored.
    ///
    /// # Errors
    ///
    /// Returns [`NoAddress`] if `forge` is [`Forge::GitHub`] or
    /// [`Forge::GitLab`] and `address` is `None`.
    pub fn new(forge: Forge, address: Option<&str>) -> Result<Self, NoAddress> {
        let address = match forge {
            Forge::None => None,
            Forge::GitHub | Forge::GitLab => Some(
                address
                    .map(|address| address.trim_end_matches('/').to_owned())
                    .ok_or(NoAddress(forge))?,
            ),
        };

        Ok(Self { forge, address })
    }

    #[must_use]
    pub const fn forge(&self) -> Forge {
        self.forge
    }

    /// Build the [`Credit`] for a commit with `hash` and `message`, preferring
    /// a change request extracted from `message` and falling back to the commit
    /// hash.
    #[must_use]
    pub fn credit(&self, hash: &str, message: &str) -> Credit {
        match self.forge.change_request(message) {
            Some(number) => self.change_request(number),
            None => self.commit(hash),
        }
    }

    /// Build the [`Credit`] for change request `number`.
    #[must_use]
    pub fn change_request(&self, number: NonZeroU64) -> Credit {
        let (label, url) = match (self.forge, &self.address) {
            (Forge::GitHub, Some(address)) => (
                format!("#{number}"),
                Some(format!("{address}/pull/{number}")),
            ),
            (Forge::GitLab, Some(address)) => (
                format!("!{number}"),
                Some(format!("{address}/-/merge_requests/{number}")),
            ),
            _ => (format!("#{number}"), None),
        };

        Credit {
            kind: CreditKind::ChangeRequest,
            label,
            url,
        }
    }

    /// Build the [`Credit`] for commit `hash`, labeled with its 7-character
    /// short hash.
    #[must_use]
    pub fn commit(&self, hash: &str) -> Credit {
        let url = match (self.forge, &self.address) {
            (Forge::GitHub, Some(address)) => Some(format!("{address}/commit/{hash}")),
            (Forge::GitLab, Some(address)) => Some(format!("{address}/-/commit/{hash}")),
            _ => None,
        };

        Credit {
            kind: CreditKind::Commit,
            label: hash.chars().take(SHORT_HASH).collect(),
            url,
        }
    }
}

/// Error returned when a forge requires a repository web URL for credit links
/// and none is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoAddress(pub Forge);

impl fmt::Display for NoAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the forge is `{}`, but git's `origin` remote has no web address for credits to link \
             to",
            self.0
        )
    }
}

impl std::error::Error for NoAddress {}

/// Attribution entry for a fragment or highlight in a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Credit {
    pub kind: CreditKind,

    /// Display label (`#1182` on GitHub, `!1182` on GitLab, or a 7-character
    /// commit hash such as `4e1f0a2`).
    pub label: String,

    /// Web URL for the change request or commit, or `None` when the forge is
    /// [`Forge::None`].
    pub url: Option<String>,
}

/// Whether a [`Credit`] refers to a change request or a commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CreditKind {
    ChangeRequest,
    Commit,
}

/// Extract the hostname (without port) from an `http://` or `https://` URL.
fn host(address: &str) -> Option<&str> {
    let rest = address
        .strip_prefix("https://")
        .or_else(|| address.strip_prefix("http://"))?;
    let authority = rest.split('/').next()?;
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = host.split(':').next()?;

    (!host.is_empty()).then_some(host)
}

/// Extract a GitHub pull request number from `(#123)` at the end of a squash
/// commit subject or `Merge pull request #123` at the start of a merge commit
/// subject.
fn github_change_request(message: &str) -> Option<NonZeroU64> {
    let subject = message.lines().next().unwrap_or_default().trim();

    let squashed = subject
        .strip_suffix(')')
        .and_then(|rest| rest.rsplit_once("(#"))
        .and_then(|(_, number)| positive(number));
    if squashed.is_some() {
        return squashed;
    }

    subject
        .strip_prefix("Merge pull request #")
        .and_then(|rest| rest.split(' ').next())
        .and_then(positive)
}

/// Extract a GitLab merge request number from the last `See merge request
/// <project>!123` trailer line.
fn gitlab_change_request(message: &str) -> Option<NonZeroU64> {
    message.lines().rev().find_map(|line| {
        let reference = line.trim().strip_prefix("See merge request ")?;
        let (_, number) = reference.trim().rsplit_once('!')?;

        positive(number)
    })
}

fn positive(text: &str) -> Option<NonZeroU64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }

    text.parse().ok()
}

#[cfg(test)]
#[path = "forge_tests.rs"]
mod tests;
