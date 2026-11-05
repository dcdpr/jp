//! The change requests `cut` and `pick` open, to file their notes on `main`.
//!
//! On GitHub, a pull request opened through its API, with the configured labels
//! added once it exists.
//! On GitLab, a merge request the push itself opens, through push options.
//! Without a forge, the pushed branch, for the project to merge however it
//! merges.
//!
//! [`Opener::new`] gathers what opening one needs before anything is pushed, so
//! a missing token stops a run before it changes `origin`.

use changelog::Forge;
use jp_github::Octocrab;
use tokio::runtime::Builder;

use crate::repository::MAIN;

/// A change request to open: a pushed branch, and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChangeRequest {
    /// The branch it merges into `main`.
    pub branch: String,

    /// Its title, on one line.
    pub title: String,

    /// Its description, as Markdown.
    pub body: String,
}

/// How a project's change requests are opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Opener {
    /// Through GitHub's API at `api`, for the repository `owner/repo`.
    GitHub {
        api: String,
        owner: String,
        repo: String,
        token: String,
        labels: Vec<String>,
    },

    /// Through the push, with GitLab's push options.
    GitLab { labels: Vec<String> },

    /// Not at all: the branch is pushed for the project to merge.
    None,
}

impl Opener {
    /// The opener for `forge`, whose repository is at the web `address` of
    /// `origin`, giving every change request `labels`.
    ///
    /// GitHub's API takes `token`.
    /// The error says what is missing.
    pub(crate) fn new(
        forge: Forge,
        address: Option<&str>,
        token: Option<String>,
        labels: Vec<String>,
    ) -> Result<Self, String> {
        match forge {
            Forge::GitHub => {
                let address = address.ok_or(
                    "The forge is `github`, but git's `origin` remote has no web address to find \
                     the repository at.",
                )?;
                let (api, owner, repo) = github(address)?;
                let token = token.ok_or(
                    "Opening the pull request needs a GitHub token; set `JP_GITHUB_TOKEN` or \
                     `GITHUB_TOKEN`.",
                )?;

                Ok(Self::GitHub {
                    api,
                    owner,
                    repo,
                    token,
                    labels,
                })
            }
            Forge::GitLab => Ok(Self::GitLab { labels }),
            Forge::None => Ok(Self::None),
        }
    }

    /// The push options the push of `request`'s branch carries.
    pub(crate) fn push_options(&self, request: &ChangeRequest) -> Vec<String> {
        let Self::GitLab { labels } = self else {
            return vec![];
        };

        let mut options = vec![
            "merge_request.create".to_owned(),
            format!("merge_request.target={MAIN}"),
            format!("merge_request.title={}", request.title),
        ];
        options.extend(
            labels
                .iter()
                .map(|label| format!("merge_request.label={label}")),
        );

        options
    }

    /// Open `request`, whose branch was pushed, and say what became of it.
    ///
    /// `pushed` is what the push printed, where GitLab names the merge request
    /// it opened.
    /// The error says why the change request couldn't be opened.
    pub(crate) fn open(&self, request: &ChangeRequest, pushed: &str) -> Result<String, String> {
        match self {
            Self::GitHub {
                api,
                owner,
                repo,
                token,
                labels,
            } => {
                let runtime = Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| format!("failed to start the async runtime: {error}"))?;
                let url = runtime
                    .block_on(open_pull_request(api, owner, repo, token, labels, request))?;

                Ok(format!("Opened {url}"))
            }
            Self::GitLab { .. } => Ok(match merge_request(pushed) {
                Some(url) => format!("Opened {url}"),
                None => format!(
                    "Asked GitLab to open a merge request from {} into {MAIN}",
                    request.branch
                ),
            }),
            Self::None => Ok(format!("Merge {} into {MAIN} to file them", request.branch)),
        }
    }
}

/// Open `request` as a pull request on `owner/repo` through the API at `api`,
/// add `labels` to it, and return its address.
async fn open_pull_request(
    api: &str,
    owner: &str,
    repo: &str,
    token: &str,
    labels: &[String],
    request: &ChangeRequest,
) -> Result<String, String> {
    let client = Octocrab::builder()
        .personal_token(token)
        .base_url(api)
        .build()
        .map_err(|error| format!("failed to create the GitHub client: {error}"))?;

    let pull = client
        .pulls(owner, repo)
        .create(&request.title, &request.branch, MAIN)
        .body(&request.body)
        .send()
        .await
        .map_err(|error| {
            format!(
                "Opening a pull request from {} failed: {error}",
                request.branch
            )
        })?;
    let url = pull
        .html_url
        .map_or_else(|| format!("pull request #{}", pull.number), String::from);

    if !labels.is_empty() {
        client
            .issues(owner, repo)
            .add_labels(pull.number, labels)
            .await
            .map_err(|error| format!("Opened {url}, but labelling it failed: {error}"))?;
    }

    Ok(url)
}

/// GitHub's REST API for the repository at the web `address`, and the
/// repository's owner and name.
///
/// GitHub.com serves its API at `api.github.com`, and a GitHub Enterprise
/// Server at `/api/v3` on its own host.
fn github(address: &str) -> Result<(String, String, String), String> {
    let unreadable =
        || format!("{address}, the address of git's `origin` remote, names no GitHub repository.");

    let (scheme, rest) = address.split_once("://").ok_or_else(unreadable)?;
    let (host, path) = rest.split_once('/').ok_or_else(unreadable)?;
    let (owner, repo) = path.split_once('/').ok_or_else(unreadable)?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return Err(unreadable());
    }

    let api = if host.eq_ignore_ascii_case("github.com") {
        "https://api.github.com".to_owned()
    } else {
        format!("{scheme}://{host}/api/v3")
    };

    Ok((api, owner.to_owned(), repo.to_owned()))
}

/// The address of the merge request GitLab says the push opened, among the
/// `remote:` lines of what the push printed.
fn merge_request(pushed: &str) -> Option<String> {
    pushed.lines().find_map(|line| {
        line.trim()
            .strip_prefix("remote:")?
            .split_whitespace()
            // GitLab links to `/-/merge_requests/new` when it opened none.
            .find(|word| {
                word.contains("/-/merge_requests/") && !word.contains("/-/merge_requests/new")
            })
            .map(ToOwned::to_owned)
    })
}

#[cfg(test)]
#[path = "change_request_tests.rs"]
mod tests;
