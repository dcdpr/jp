//! Reading GitHub issues into tickets.
//!
//! [`import`] is the entry point.
//! The traffic is one way: replies belong on GitHub and arrive on the next
//! import, so nothing is ever written back.
//! Each import replaces the ticket's content and leaves its metadata block
//! alone, so triage done here survives the next one.

use camino::Utf8Path;
use chrono::SecondsFormat;
use jp_github::models::issues::{Comment as IssueComment, Issue};
use ticket::{
    Comment, Kind,
    import::{Import, Source},
    store::{self, Existing, Outcome},
};

use crate::Output;

/// Comments per request; the GitHub maximum, so long threads take few round
/// trips.
const PER_PAGE: u8 = 100;

/// Fetch an issue and write it into a ticket, creating the ticket on the first
/// import and refreshing it after that.
pub fn import(dir: &Utf8Path, number: u64, repo: &str, kind: Kind) -> Result<Output, String> {
    let (owner, name) = split_repo(repo)?;
    let (issue, comments) = runtime()?.block_on(fetch(owner, name, number))?;

    if issue.pull_request.is_some() {
        return Err(format!("{repo}#{number} is a pull request, not an issue."));
    }

    let comments: Vec<Comment> = comments
        .into_iter()
        .map(|comment| Comment {
            from: format!("gh:{}", comment.user.login),
            date: comment
                .created_at
                .to_rfc3339_opts(SecondsFormat::Secs, true),
            re: None,
            body: comment.body.unwrap_or_default(),
        })
        .collect();

    let count = comments.len();
    let imported = store::import(
        dir,
        &Import {
            source: Source::GitHub { number },
            title: &issue.title,
            description: issue.body.as_deref().unwrap_or_default(),
            comments,
            kind,
            authors: &format!("gh:{}", issue.user.login),
            date: &issue.created_at.format("%Y-%m-%d").to_string(),
        },
        Existing::Refresh,
    )
    .map_err(|error| error.to_string())?;

    let verb = match imported.outcome {
        Outcome::Created => "Imported",
        Outcome::Refreshed | Outcome::Skipped => "Refreshed",
    };

    Ok(format!(
        "{verb} {} from {repo}#{number} at {} ({count} comments)\n",
        imported.id, imported.path
    )
    .into())
}

/// Read a repository's open issues.
pub fn open_issues(repo: &str) -> Result<Vec<Issue>, String> {
    let (owner, name) = split_repo(repo)?;

    runtime()?.block_on(async {
        client()?
            .issues(owner, name)
            .list()
            .per_page(PER_PAGE)
            .send()
            .await
            .map_err(|error| format!("failed to list issues in {repo}: {error}"))
    })
}

/// Read an issue and every page of its comments.
async fn fetch(owner: &str, repo: &str, number: u64) -> Result<(Issue, Vec<IssueComment>), String> {
    let client = client()?;
    let issues = client.issues(owner, repo);
    let issue = issues
        .get(number)
        .await
        .map_err(|error| format!("failed to read {owner}/{repo}#{number}: {error}"))?;

    let mut comments = vec![];
    for page in 1.. {
        let batch = issues
            .list_comments(number)
            .page(page)
            .per_page(PER_PAGE)
            .send()
            .await
            .map_err(|error| format!("failed to read comments on #{number}: {error}"))?;

        let short = batch.len() < usize::from(PER_PAGE);
        comments.extend(batch);
        if short {
            break;
        }
    }

    Ok((issue, comments))
}

/// Build a client, authenticated when a token is around.
///
/// Anonymous requests work against public repositories, at a much lower rate
/// limit; a token raises it and reaches private ones.
fn client() -> Result<jp_github::Octocrab, String> {
    let mut builder = jp_github::Octocrab::builder();
    if let Some(token) = token() {
        builder = builder.personal_token(token);
    }

    builder
        .build()
        .map_err(|error| format!("failed to create the GitHub client: {error}"))
}

fn token() -> Option<String> {
    let non_empty = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());

    non_empty("JP_GITHUB_TOKEN").or_else(|| non_empty("GITHUB_TOKEN"))
}

/// A runtime to drive the requests from an otherwise synchronous plugin.
fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("failed to start the async runtime: {error}"))
}

fn split_repo(repo: &str) -> Result<(&str, &str), String> {
    repo.split_once('/')
        .ok_or_else(|| format!("`{repo}` is not an `owner/name` pair."))
}
