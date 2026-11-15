//! GitHub repository coordinates and API client construction.

use std::process::Command;

use camino::Utf8Path;
use doc::repository::web_address;
use jp_github::Octocrab;

/// Public GitHub hostname (`api.github.com`); any other host is treated as a
/// GitHub Enterprise instance serving its API at `https://<host>/api/v3`.
const GITHUB: &str = "github.com";

/// Parsed GitHub repository coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Repository {
    /// Base web URL (`https://<host>/<owner>/<name>`).
    pub(crate) web: String,
    pub(crate) host: String,
    pub(crate) owner: String,
    pub(crate) name: String,
}

impl Repository {
    /// Parse `https://<host>/<owner>/<name>` into a [`Repository`].
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

    /// Resolve the target GitHub repository from `configured`
    /// (`plugins.command.release.options.repository`) or git's `origin` remote
    /// in `root`.
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

    /// Return the custom API base URL (`https://<host>/api/v3`) for GitHub
    /// Enterprise hosts, or `None` for `github.com`.
    pub(crate) fn api(&self) -> Option<String> {
        (self.host != GITHUB).then(|| format!("https://{}/api/v3", self.host))
    }

    /// Return the asset download base URL for `tag`
    /// (`https://<host>/<owner>/<name>/releases/download/<tag>`).
    pub(crate) fn download(&self, tag: &str) -> String {
        format!("{}/releases/download/{tag}", self.web)
    }

    /// Build an authenticated [`Octocrab`] client for this repository's host.
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
