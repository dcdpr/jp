//! Where a project's changes are reviewed, which decides how a release credits
//! them and where the credits link.
//!
//! A release credits each fragment and highlight to the change requests that
//! wrote it, read from the messages of the commits that brought them to the
//! default branch, and to a commit when its message names none:
//!
//! | Forge    | A change request is read from                                           | Label  | Links to                                        |
//! | -------- | ----------------------------------------------------------------------- | ------ | ----------------------------------------------- |
//! | `github` | `(#123)` ending a squash commit's subject, or `Merge pull request #123` | `#123` | `https://<host>/<owner>/<repo>/pull/123`        |
//! | `gitlab` | a line `See merge request <project>!123`                                | `!123` | `https://<host>/<project>/-/merge_requests/123` |
//! | `none`   | nothing                                                                 |        |                                                 |
//!
//! A commit is labelled with its short hash and links to `/commit/<hash>` on
//! GitHub, `/-/commit/<hash>` on GitLab, and nowhere without a forge.

use std::{fmt, num::NonZeroU64, str::FromStr};

use serde::Serialize;

/// How many characters of a commit's hash its label shows.
const SHORT_HASH: usize = 7;

/// The forge a project's change requests are reviewed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forge {
    /// GitHub, github.com or a host of its own: pull requests.
    GitHub,

    /// GitLab, gitlab.com or a host of its own: merge requests.
    GitLab,

    /// No forge: every credit is a commit, and none links anywhere.
    None,
}

impl Forge {
    /// Every forge, as configuration names them.
    pub const ALL: [Self; 3] = [Self::GitHub, Self::GitLab, Self::None];

    /// The name configuration gives the forge, such as `github`.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::GitHub => "github",
            Self::GitLab => "gitlab",
            Self::None => "none",
        }
    }

    /// The forge a project's web address is on: GitHub for github.com, GitLab
    /// for gitlab.com, and none for any other host, or no address at all.
    ///
    /// A host of GitHub's or GitLab's own can't be told from its address, so a
    /// project on one names its forge.
    #[must_use]
    pub fn infer(address: Option<&str>) -> Self {
        let host = address.and_then(host).map(str::to_ascii_lowercase);

        match host.as_deref() {
            Some("github.com") => Self::GitHub,
            Some("gitlab.com") => Self::GitLab,
            _ => Self::None,
        }
    }

    /// The change request a commit's `message` names, as the forge writes it
    /// into the commits it merges.
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

    /// Read a forge from its name, written as configuration writes it.
    fn from_str(id: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|forge| forge.id() == id)
            .ok_or_else(|| UnknownForge(id.to_owned()))
    }
}

/// A string that names none of the forges.
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

/// A project on its forge: what its credits read like, and where they link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    forge: Forge,

    /// The project's web address, such as `https://github.com/dcdpr/jp`, which
    /// links start from; `None` without a forge.
    address: Option<String>,
}

impl Project {
    /// The project at the web `address` on `forge`, with no `/` at its end.
    ///
    /// Without a forge the address is unused, since nothing links.
    ///
    /// # Errors
    ///
    /// Returns [`NoAddress`] when `forge` is GitHub or GitLab and there is no
    /// address for links to start from.
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

    /// The credit for the commit `hash`, whose message is `message`: the change
    /// request the message names, or else the commit itself.
    #[must_use]
    pub fn credit(&self, hash: &str, message: &str) -> Credit {
        match self.forge.change_request(message) {
            Some(number) => self.change_request(number),
            None => self.commit(hash),
        }
    }

    /// The credit for change request `number`.
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

    /// The credit for the commit `hash`, labelled with its short hash.
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

/// A forge that links to its change requests, with no web address to link at.
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

/// Who a release credits for a fragment or highlight: a change request, or a
/// commit that reached the default branch without one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Credit {
    pub kind: CreditKind,

    /// The credit as the forge writes it: `#1182` on GitHub, `!1182` on GitLab,
    /// or a commit's short hash, such as `4e1f0a2`.
    pub label: String,

    /// The credit's page on the forge; `None` without a forge.
    pub url: Option<String>,
}

/// What a [`Credit`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CreditKind {
    ChangeRequest,
    Commit,
}

/// The host of a web address, such as `github.com`, without a port.
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

/// The pull request a GitHub commit names: `(#123)` ending a squash commit's
/// subject, or `Merge pull request #123` opening a merge commit's.
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

/// The merge request a GitLab commit names: the last line reading `See merge
/// request <project>!123`, as GitLab ends the message of a merge commit.
fn gitlab_change_request(message: &str) -> Option<NonZeroU64> {
    message.lines().rev().find_map(|line| {
        let reference = line.trim().strip_prefix("See merge request ")?;
        let (_, number) = reference.trim().rsplit_once('!')?;

        positive(number)
    })
}

/// `text` as a positive number, when it is nothing but ASCII digits.
fn positive(text: &str) -> Option<NonZeroU64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }

    text.parse().ok()
}

#[cfg(test)]
#[path = "forge_tests.rs"]
mod tests;
