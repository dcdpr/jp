//! The repository on GitHub a workspace publishes its releases to, and a client
//! for GitHub's API there.

use std::process::Command;

use camino::Utf8Path;
use doc::repository::web_address;
use jp_github::Octocrab;

/// The host of GitHub itself, whose API is at `api.github.com`; any other host
/// is a GitHub Enterprise server, with its API below `/api/v3`.
const GITHUB: &str = "github.com";

/// A repository on GitHub, read from its web address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Repository {
    /// The repository's web address, such as `https://github.com/dcdpr/jp`.
    pub(crate) web: String,

    /// The host, such as `github.com`.
    pub(crate) host: String,

    pub(crate) owner: String,
    pub(crate) name: String,
}

impl Repository {
    /// The repository at `address`, an `https://<host>/<owner>/<name>` address;
    /// `None` for one with a path of any other length.
    pub(crate) fn parse(address: &str) -> Option<Self> {
        let rest = address.strip_prefix("https://")?;
        let mut parts = rest.trim_end_matches('/').split('/');
        let (Some(host), Some(owner), Some(name), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        if [host, owner, name].iter().any(|part| part.is_empty()) {
            return None;
        }

        Some(Self {
            web: format!("https://{host}/{owner}/{name}"),
            host: host.to_owned(),
            owner: owner.to_owned(),
            name: name.to_owned(),
        })
    }

    /// The repository the workspace at `root` publishes to: at `configured`,
    /// `jp release`'s `repository`, or else at git's `origin` remote's web
    /// address.
    pub(crate) fn find(root: &Utf8Path, configured: Option<&str>) -> Result<Self, String> {
        let address = match configured {
            Some(address) => address.to_owned(),
            None => origin(root).ok_or_else(|| {
                "There is no repository to publish to: set \
                 `plugins.command.release.options.repository` to its address, such as \
                 `https://github.com/dcdpr/jp`, or give the checkout an `origin` remote on GitHub."
                    .to_owned()
            })?,
        };

        Self::parse(&address).ok_or_else(|| {
            format!(
                "`{address}` isn't the address of a repository on GitHub, which reads as \
                 `https://github.com/<owner>/<name>`."
            )
        })
    }

    /// Where GitHub's API serves this repository's host from: `None` for GitHub
    /// itself, and `https://<host>/api/v3` for GitHub Enterprise.
    pub(crate) fn api(&self) -> Option<String> {
        (self.host != GITHUB).then(|| format!("https://{}/api/v3", self.host))
    }

    /// The address GitHub serves the files attached to the Release of `tag`
    /// from.
    pub(crate) fn download(&self, tag: &str) -> String {
        format!("{}/releases/download/{tag}", self.web)
    }

    /// A client for GitHub's API on this repository's host, sending `token`.
    pub(crate) fn client(&self, token: &str) -> Result<Octocrab, String> {
        let mut builder = Octocrab::builder().personal_token(token.to_owned());
        if let Some(api) = self.api() {
            builder = builder.base_url(api);
        }

        builder
            .build()
            .map_err(|error| format!("failed to create the GitHub client: {error}"))
    }
}

/// The web address of the `origin` remote of the repository at `root`, if it
/// has one an address can be read from.
fn origin(root: &Utf8Path) -> Option<String> {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    web_address(String::from_utf8_lossy(&output.stdout).trim())
}

#[cfg(test)]
#[path = "github_tests.rs"]
mod tests;
